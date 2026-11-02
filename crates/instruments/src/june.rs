// SPDX-License-Identifier: GPL-3.0-or-later
//! June: the six-voice polysynth with a chorus, built the way the 1984
//! Japanese classic was (`docs/plans/classic-synths.md`, S4), for pads,
//! chords, house stabs and bass. Each voice has one digitally
//! controlled oscillator giving a saw and a pulse at once (the pulse's
//! width moved by hand, by the LFO or by the envelope), a square
//! sub-oscillator an octave down, and noise; a four-position high-pass
//! whose first position lifts the bass instead; the 24 dB
//! transconductance cascade low-pass that sings on its own at the top
//! of its resonance, moved by the envelope (either way up), the LFO,
//! the keyboard and the bender; one ADSR envelope with analogue curves
//! shared by the filter and the amplifier, which can follow the
//! envelope or just the gate; and a triangle LFO with a delay before it
//! fades in. The voices sum into the stereo bucket-brigade chorus,
//! I, II or both at once.
//!
//! Everything nonlinear is where the circuit has it: the filter's input
//! and feedback saturate (the toolkit's [`Cascade`], run at twice the
//! sample rate) and the output limits softly. The oscillators hold
//! their pitch, as a digitally controlled oscillator does; what drifts
//! with **Vintage drift** is each voice's pulse width, level and filter
//! cutoff, as the trimmed parts of the circuit do. Six voices by
//! default, up to sixteen; polyphonic, stacked in unison, or one note.

use daw_engine::midi::{Event, Instrument, Kind};

use crate::dsp::{Noise, db, hz};
use crate::params::{Live, Param, Unit};
use crate::vintage::{
    Action, Allocator, Analog, Cascade, Chorus, ChorusMode, Drift, Glide, GlideMode, Highpass,
    OnePole, Osc, Poles, Priority, Shape, Steps, Sub, SubShape, VoiceSettings, Wave, onepole, soft,
};

const RANGES: &[&str] = &["16' (an octave down)", "8'", "4' (an octave up)"];
const PWM_SOURCES: &[&str] = &["By hand", "LFO", "Envelope"];
/// The high-pass switch: the first position lifts the bass a little
/// rather than cutting, as the circuit's does.
const HIGHPASS: &[&str] = &["Bass lift", "Flat", "Cut at 250 Hz", "Cut at 600 Hz"];
const AMP_MODES: &[&str] = &["Envelope", "Gate"];
const MODES: &[&str] = &["Poly", "Unison", "Mono"];
const PORTAMENTO: &[&str] = &["Off", "On"];

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
    pub const NOISE: usize = 8;
    pub const HIGHPASS: usize = 9;
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
    pub const LFO_RATE: usize = 20;
    pub const LFO_DELAY: usize = 21;
    pub const CHORUS: usize = 22;
    pub const MODE: usize = 23;
    pub const VOICES: usize = 24;
    pub const UNISON_DETUNE: usize = 25;
    pub const PORTAMENTO: usize = 26;
    pub const GLIDE: usize = 27;
    pub const BEND: usize = 28;
    pub const BEND_FILTER: usize = 29;
    pub const WHEEL: usize = 30;
    pub const VELOCITY: usize = 31;
    pub const VELOCITY_FILTER: usize = 32;
    pub const LEVEL: usize = 33;
    pub const DRIVE: usize = 34;
    pub const VINTAGE: usize = 35;
}

/// The synth's settings, in the order of [`at`]. Grouped for showing by
/// [`GROUPS`].
pub fn params() -> Vec<Param> {
    use Unit::*;
    vec![
        Param::new("range", "Range", 0.0, 2.0, 1.0, Choice).choices(RANGES),
        Param::new("lfo_pitch", "LFO to pitch", 0.0, 12.0, 0.0, Semitones),
        Param::new("width", "Pulse width", 0.05, 0.95, 0.5, Amount),
        Param::new("pwm_source", "Width moved by", 0.0, 2.0, 1.0, Choice).choices(PWM_SOURCES),
        Param::new("pwm", "Width movement", 0.0, 1.0, 0.0, Amount),
        Param::new("saw", "Saw", 0.0, 1.0, 1.0, Amount),
        Param::new("pulse", "Pulse", 0.0, 1.0, 0.0, Amount),
        Param::new("sub", "Sub oscillator", 0.0, 1.0, 0.3, Amount),
        Param::new("noise", "Noise", 0.0, 1.0, 0.0, Amount),
        Param::new("highpass", "High-pass", 0.0, 3.0, 1.0, Choice).choices(HIGHPASS),
        Param::new("cutoff", "Cutoff", 10.0, 20_000.0, 1_200.0, Hz).log(),
        Param::new("resonance", "Resonance", 0.0, 1.0, 0.1, Amount),
        Param::new("filter_env", "Envelope to filter", -6.0, 6.0, 2.0, Octaves),
        Param::new("filter_lfo", "LFO to filter", 0.0, 4.0, 0.0, Octaves),
        Param::new("key_track", "Keyboard to filter", 0.0, 1.0, 0.5, Amount),
        Param::new("amp", "Amplifier follows", 0.0, 1.0, 0.0, Choice).choices(AMP_MODES),
        Param::new("attack", "Attack", 0.0015, 5.0, 0.005, Seconds).log(),
        Param::new("decay", "Decay", 0.002, 12.0, 0.5, Seconds).log(),
        Param::new("sustain", "Sustain", 0.0, 1.0, 0.6, Amount),
        Param::new("release", "Release", 0.002, 12.0, 0.3, Seconds).log(),
        Param::new("lfo_rate", "LFO rate", 0.1, 30.0, 4.0, Hz).log(),
        Param::new("lfo_delay", "LFO delay", 0.0, 4.0, 0.0, Seconds),
        Param::new("chorus", "Chorus", 0.0, 3.0, 0.0, Choice).choices(ChorusMode::NAMES),
        Param::new("mode", "Playing", 0.0, 2.0, 0.0, Choice).choices(MODES),
        Param::new("voices", "Voices", 1.0, MAX_VOICES as f64, 6.0, Count),
        Param::new("unison_detune", "Unison detune", 0.0, 50.0, 8.0, Cents),
        Param::new("portamento", "Portamento", 0.0, 1.0, 0.0, Choice).choices(PORTAMENTO),
        Param::new("glide", "Portamento time", 0.005, 3.0, 0.1, Seconds).log(),
        Param::new("bend", "Bend range", 1.0, 24.0, 2.0, Semitones),
        Param::new("bend_filter", "Bend to filter", 0.0, 4.0, 0.0, Octaves),
        Param::new("wheel", "Mod wheel vibrato", 0.0, 12.0, 0.5, Semitones),
        Param::new("velocity", "Velocity", 0.0, 1.0, 0.3, Amount),
        Param::new(
            "velocity_filter",
            "Velocity to filter",
            0.0,
            4.0,
            0.0,
            Octaves,
        ),
        Param::new("level", "Level", -36.0, 6.0, -6.0, Db),
        Param::new("drive", "Drive", 0.0, 1.0, 0.0, Amount),
        Param::new("vintage", "Vintage drift", 0.0, 1.0, 0.3, Amount),
    ]
}

/// The settings in groups, for the controls: (heading, first, last).
pub const GROUPS: &[(&str, usize, usize)] = &[
    ("Oscillator and mixer", at::RANGE, at::NOISE),
    ("Filter", at::HIGHPASS, at::KEY_TRACK),
    ("Envelope and amplifier", at::AMP, at::RELEASE),
    ("LFO and chorus", at::LFO_RATE, at::CHORUS),
    ("Playing", at::MODE, at::LEVEL),
    ("Character", at::DRIVE, at::VINTAGE),
];

/// The sounds it makes from its settings.
pub fn sounds() -> Vec<crate::Sound> {
    use crate::Sound;
    vec![
        Sound {
            name: "Strings pad",
            settings: &[
                ("saw", 1.0),
                ("sub", 0.0),
                ("highpass", 2.0),
                ("cutoff", 900.0),
                ("resonance", 0.05),
                ("filter_env", 0.8),
                ("attack", 0.6),
                ("decay", 1.0),
                ("sustain", 0.8),
                ("release", 1.2),
                ("chorus", 1.0),
                ("lfo_rate", 5.0),
                ("lfo_delay", 1.0),
                ("lfo_pitch", 0.15),
            ],
        },
        Sound {
            name: "Warm pad",
            settings: &[
                ("saw", 0.0),
                ("pulse", 1.0),
                ("pwm_source", 1.0),
                ("pwm", 0.6),
                ("sub", 0.3),
                ("lfo_rate", 0.7),
                ("cutoff", 600.0),
                ("resonance", 0.1),
                ("filter_env", 1.0),
                ("attack", 1.0),
                ("decay", 2.0),
                ("sustain", 0.7),
                ("release", 2.0),
                ("chorus", 3.0),
            ],
        },
        Sound {
            name: "House stab",
            settings: &[
                ("saw", 1.0),
                ("pulse", 0.6),
                ("sub", 0.4),
                ("cutoff", 500.0),
                ("resonance", 0.25),
                ("filter_env", 3.0),
                ("attack", 0.002),
                ("decay", 0.25),
                ("sustain", 0.0),
                ("release", 0.15),
                ("chorus", 2.0),
                ("velocity", 0.5),
                ("velocity_filter", 1.0),
            ],
        },
        Sound {
            name: "Chorus bass",
            settings: &[
                ("range", 0.0),
                ("saw", 1.0),
                ("sub", 0.7),
                ("cutoff", 350.0),
                ("resonance", 0.2),
                ("filter_env", 2.5),
                ("key_track", 0.3),
                ("attack", 0.002),
                ("decay", 0.35),
                ("sustain", 0.2),
                ("release", 0.12),
                ("chorus", 1.0),
                ("mode", 2.0),
                ("velocity", 0.4),
            ],
        },
        Sound {
            name: "Round bass",
            settings: &[
                ("range", 0.0),
                ("saw", 0.0),
                ("pulse", 1.0),
                ("width", 0.5),
                ("pwm_source", 0.0),
                ("sub", 0.8),
                ("highpass", 0.0),
                ("cutoff", 220.0),
                ("resonance", 0.15),
                ("filter_env", 2.0),
                ("key_track", 0.4),
                ("attack", 0.003),
                ("decay", 0.4),
                ("sustain", 0.3),
                ("release", 0.1),
                ("mode", 2.0),
                ("drive", 0.25),
            ],
        },
        Sound {
            name: "Brass",
            settings: &[
                ("saw", 1.0),
                ("sub", 0.0),
                ("cutoff", 400.0),
                ("resonance", 0.3),
                ("filter_env", 3.0),
                ("key_track", 0.6),
                ("attack", 0.08),
                ("decay", 0.4),
                ("sustain", 0.5),
                ("release", 0.25),
                ("chorus", 1.0),
                ("velocity", 0.5),
                ("velocity_filter", 1.0),
            ],
        },
        Sound {
            name: "Organ",
            settings: &[
                ("saw", 0.0),
                ("pulse", 0.8),
                ("width", 0.5),
                ("pwm_source", 0.0),
                ("sub", 1.0),
                ("cutoff", 3_000.0),
                ("resonance", 0.0),
                ("filter_env", 0.0),
                ("amp", 1.0),
                ("chorus", 2.0),
                ("velocity", 0.0),
            ],
        },
        Sound {
            name: "Unison lead",
            settings: &[
                ("saw", 1.0),
                ("pulse", 0.5),
                ("pwm", 0.3),
                ("sub", 0.2),
                ("cutoff", 1_500.0),
                ("resonance", 0.2),
                ("filter_env", 1.5),
                ("attack", 0.01),
                ("decay", 0.5),
                ("sustain", 0.7),
                ("release", 0.3),
                ("mode", 1.0),
                ("unison_detune", 12.0),
                ("portamento", 1.0),
                ("glide", 0.08),
                ("lfo_delay", 0.6),
                ("lfo_pitch", 0.2),
                ("wheel", 1.0),
            ],
        },
    ]
}

/// The most voices there are.
pub const MAX_VOICES: usize = 16;

/// Settings are read, and the filters set, this often: a third of a
/// millisecond, so a fast attack still sweeps the filter in steps too
/// small to hear.
const CHUNK: usize = 16;

/// The gate's amplifier shape: on and off in a few milliseconds, so it
/// doesn't click.
fn gate_shape() -> Shape {
    Shape::adsr(0.002, 0.001, 1.0, 0.004)
}

/// The bass lift of the high-pass switch's first position: this much of
/// a low-pass at this corner added back.
const LIFT: f32 = 0.4;
const LIFT_HZ: f32 = 180.0;

/// One voice.
struct Voice {
    saw: Osc,
    pulse: Osc,
    sub: Sub,
    highpass: Highpass,
    lift: OnePole,
    filter: Cascade,
    env: Analog,
    gate: Analog,
    drift: Drift,
    on: bool,
    /// The pitch now, gliding towards `target`.
    pitch: f32,
    target: f32,
    velocity: f32,
    /// Where in a unison stack it sits, −1 to 1, for its detune.
    stack: f32,
}

impl Voice {
    fn new(seed: u32) -> Voice {
        let mut sub = Sub::new(SubShape::OctaveDown);
        sub.reset();
        Voice {
            saw: Osc::new(Wave::Saw),
            pulse: Osc::new(Wave::Pulse),
            sub,
            highpass: Highpass::new(),
            lift: OnePole::default(),
            filter: Cascade::new(),
            env: Analog::IDLE,
            gate: Analog::IDLE,
            drift: Drift::new(seed),
            on: false,
            pitch: 60.0,
            target: 60.0,
            velocity: 1.0,
            stack: 0.0,
        }
    }
}

/// The synth on the audio thread.
pub struct June {
    params: Live,
    rate: f32,
    alloc: Allocator,
    voices: Vec<Voice>,
    noise: Noise,
    chorus: Chorus,
    bend: f32,
    wheel: f32,
    /// Keys down, to know when the LFO's delay restarts.
    keys_down: u32,
    lfo_phase: f32,
    /// How far the LFO has faded in after its delay, 0 to 1, and the
    /// time since the first key went down.
    lfo_fade: f32,
    lfo_since: f32,
    actions: Vec<Action>,
    /// The chunk's mono mix, before the chorus.
    mix: [f32; CHUNK],
}

impl June {
    pub fn new(rate: u32, params: impl Into<Live>) -> June {
        June {
            params: params.into(),
            rate: rate.max(1) as f32,
            alloc: Allocator::new(MAX_VOICES),
            voices: (0..MAX_VOICES as u32).map(|i| Voice::new(i + 1)).collect(),
            noise: Noise::new(0x0606_1984),
            chorus: Chorus::new(rate),
            bend: 0.0,
            wheel: 0.0,
            keys_down: 0,
            lfo_phase: 0.25,
            lfo_fade: 1.0,
            lfo_since: 0.0,
            actions: Vec::with_capacity(2 * MAX_VOICES),
            mix: [0.0; CHUNK],
        }
    }

    fn settings(&self) -> VoiceSettings {
        let voices = (self.params.read(at::VOICES) as usize).clamp(1, MAX_VOICES);
        match self.params.read(at::MODE) as u32 {
            1 => VoiceSettings {
                notes: 1,
                unison: voices,
                priority: Priority::Last,
                legato: false,
            },
            2 => VoiceSettings {
                notes: 1,
                unison: 1,
                priority: Priority::Last,
                legato: false,
            },
            _ => VoiceSettings {
                notes: voices,
                unison: 1,
                priority: Priority::Last,
                legato: false,
            },
        }
    }

    /// How many voices sound at once: the allocator is sized to the
    /// setting whenever every voice is quiet, so a six-voice synth has
    /// six voices and no more, release tails included.
    fn resize(&mut self) {
        let want = (self.params.read(at::VOICES) as usize).clamp(1, MAX_VOICES);
        if self.alloc.voices() != want && self.voices.iter().all(|v| !v.on) {
            self.alloc = Allocator::new(want);
        }
    }

    fn apply(&mut self) {
        let sens = self.params.read(at::VELOCITY);
        let portamento = self.params.read(at::PORTAMENTO) >= 0.5;
        let s = self.settings();
        for i in 0..self.actions.len() {
            match self.actions[i] {
                Action::Start {
                    voice,
                    note,
                    velocity,
                    fresh,
                } => {
                    let v = &mut self.voices[voice];
                    v.target = note as f32;
                    if fresh || !portamento {
                        v.pitch = v.target;
                    }
                    if fresh {
                        // The filter starts clean; the oscillator runs
                        // free, as the circuit's does.
                        v.filter.reset();
                        v.highpass.reset();
                        v.lift.reset();
                        v.sub.reset();
                    }
                    v.stack = if s.unison > 1 {
                        voice as f32 / (s.unison - 1) as f32 * 2.0 - 1.0
                    } else {
                        0.0
                    };
                    v.velocity = 1.0 - sens + sens * velocity.clamp(1, 127) as f32 / 127.0;
                    v.env.trigger();
                    v.gate.trigger();
                    v.on = true;
                }
                Action::Retune { voice, note } => {
                    let v = &mut self.voices[voice];
                    v.target = note as f32;
                    if !portamento {
                        v.pitch = v.target;
                    }
                }
                Action::Release { voice } => {
                    self.voices[voice].env.release();
                    self.voices[voice].gate.release();
                }
            }
        }
        self.actions.clear();
    }

    fn event(&mut self, e: &Event) {
        self.resize();
        let s = self.settings();
        match e.kind {
            Kind::NoteOn {
                pitch, velocity, ..
            } => {
                if self.keys_down == 0 {
                    // The first key down restarts the LFO and its delay.
                    self.lfo_since = 0.0;
                    self.lfo_phase = 0.25;
                }
                self.keys_down += 1;
                self.alloc.note_on(pitch, velocity, &s, &mut self.actions);
                self.apply();
            }
            Kind::NoteOff { pitch, .. } => {
                self.keys_down = self.keys_down.saturating_sub(1);
                self.alloc.note_off(pitch, &s, &mut self.actions);
                self.apply();
            }
            Kind::PitchBend(b) => self.bend = b as f32 / 8192.0,
            Kind::Control { number: 1, value } => self.wheel = value as f32 / 127.0,
            Kind::Control { number: 64, value } => {
                self.alloc.pedal(value >= 64, &mut self.actions);
                self.apply();
            }
            Kind::Control { number: 120, .. } | Kind::AllNotesOff => {
                self.keys_down = 0;
                self.alloc.all_off(&mut self.actions);
                self.apply();
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

    /// The LFO's triangle at `phase`, −1 to 1.
    #[inline]
    fn triangle(phase: f32) -> f32 {
        1.0 - 4.0 * (phase - 0.5).abs()
    }

    fn chunk(&mut self, from: usize, to: usize, left: &mut [f32], right: &mut [f32]) {
        let p = &self.params;
        let r = self.rate;
        let count = to - from;
        let n = count as f32;

        // The LFO, shared by the voices, with its delay: it fades in
        // over a quarter of the delay once the delay has passed.
        let lfo_dt = p.read(at::LFO_RATE) / r;
        let delay = p.read(at::LFO_DELAY);
        self.lfo_since += n / r;
        self.lfo_fade = if delay <= 0.0 {
            1.0
        } else {
            ((self.lfo_since - delay) / (0.25 * delay)).clamp(0.0, 1.0)
        };
        let lfo_now = June::triangle(self.lfo_phase) * self.lfo_fade;
        let lfo_start = self.lfo_phase;
        self.lfo_phase = (self.lfo_phase + lfo_dt * n).fract();
        self.chorus.mode = ChorusMode::from_index(p.read(at::CHORUS));
        if self.voices.iter().all(|v| !v.on) {
            // Nothing sounds; the chorus's tail still rings out.
            if self.chorus.mode != ChorusMode::Off {
                for i in from..to {
                    let (l, rr) = self.chorus.run(0.0);
                    left[i] += l;
                    right[i] += rr;
                }
            }
            return;
        }

        let range = match p.read(at::RANGE) as u32 {
            0 => -12.0,
            2 => 12.0,
            _ => 0.0,
        };
        let width = p.read(at::WIDTH);
        let pwm_source = p.read(at::PWM_SOURCE) as u32;
        let pwm = p.read(at::PWM);
        let saw = p.read(at::SAW);
        let pulse = p.read(at::PULSE);
        let sub = p.read(at::SUB);
        let noise = p.read(at::NOISE);
        let highpass = p.read(at::HIGHPASS) as u32;
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
        let detune = p.read(at::UNISON_DETUNE) / 100.0;
        let bend = self.bend * p.read(at::BEND);
        let bend_filter = self.bend * p.read(at::BEND_FILTER);
        let lfo_pitch = p.read(at::LFO_PITCH) + self.wheel * p.read(at::WHEEL);
        let vel_filter = p.read(at::VELOCITY_FILTER);
        let gain = db(p.read(at::LEVEL));
        let drive = 1.0 + 3.0 * p.read(at::DRIVE);
        let vintage = p.read(at::VINTAGE);
        let hp_cutoff = match highpass {
            2 => 250.0,
            3 => 600.0,
            _ => 0.0,
        };
        let lift_g = onepole::coefficient(LIFT_HZ, r).1;
        // The sources sum into the filter at a level that leaves room:
        // a saw and a sub together just start to warm its input.
        let mix_gain = 0.5;
        // Several voices of a chord sum before the chorus; the level
        // is set for a few at once.
        let voice_gain = 0.7;

        // One noise source serves every voice, as the circuit's does.
        let mut noise_buf = [0.0f32; CHUNK];
        if noise > 0.0 {
            for v in &mut noise_buf[..count] {
                *v = self.noise.tick();
            }
        }
        self.mix[..count].iter_mut().for_each(|v| *v = 0.0);

        for (vi, v) in self.voices.iter_mut().enumerate() {
            if !v.on {
                continue;
            }
            // Once a chunk: the pitch with its glide, the drift of this
            // voice's trimmed parts, and the filter's cutoff from the
            // key, the envelope, the LFO, velocity and the bender.
            v.pitch = glide.step(v.pitch, v.target, n, r);
            v.drift.next(vintage, n, r);
            let breathe = vintage * v.drift.slow();
            let offset = vintage * v.drift.offset();
            let pitch = v.pitch + range + bend + lfo_pitch * lfo_now + v.stack * detune;
            let f = hz(pitch);
            v.saw.set_hz(f, r);
            v.pulse.set_hz(f, r);
            let octaves = track * (pitch - 60.0) / 12.0
                + fenv * v.env.level
                + flfo * lfo_now
                + bend_filter
                + vel_filter * (v.velocity - 1.0)
                + 0.08 * offset;
            v.filter
                .set(Poles::Four, cutoff * octaves.exp2(), res, drive, 0.3, r);
            v.highpass.set(2, hp_cutoff, r);
            let level = v.velocity * gain * voice_gain * (1.0 + 0.04 * breathe);
            let mut lfo_phase = lfo_start;
            let mut done = false;
            for (i, &nz) in noise_buf[..count].iter().enumerate() {
                let e = v.env.next(&steps);
                let g = v.gate.next(&gate_steps);
                v.pulse.width = match pwm_source {
                    1 => {
                        let l = June::triangle(lfo_phase) * self.lfo_fade;
                        lfo_phase = (lfo_phase + lfo_dt).fract();
                        (0.5 + 0.45 * pwm * l).clamp(0.05, 0.95)
                    }
                    2 => (0.5 + 0.45 * pwm * e).clamp(0.05, 0.95),
                    _ => (width + 0.03 * breathe).clamp(0.05, 0.95),
                };
                let s = v.saw.tick(None);
                let q = v.pulse.tick(None);
                let b = v.sub.tick(v.saw.wrapped);
                let mut x = saw * s + pulse * q + sub * b + noise * nz;
                x = match highpass {
                    0 => x + LIFT * v.lift.lp(x, lift_g),
                    1 => x,
                    _ => v.highpass.run(x),
                };
                let y = v.filter.run(x * mix_gain);
                let amp = if gate_mode { g } else { e };
                self.mix[i] += y * amp * level;
                done = if gate_mode {
                    !v.gate.active()
                } else {
                    !v.env.active()
                };
                if done {
                    break;
                }
            }
            if done {
                v.on = false;
                self.alloc.free(vi);
            }
        }

        // The voices' sum through the chorus, limited softly.
        for i in 0..count {
            let (l, rr) = self.chorus.run(self.mix[i]);
            left[from + i] += soft(l * 1.5) / 1.5;
            right[from + i] += soft(rr * 1.5) / 1.5;
        }
    }
}

impl Instrument for June {
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

    fn synth() -> June {
        June::new(RATE as u32, Arc::new(Params::new(params())))
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

    fn render(s: &mut June, events: &[Event], n: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        s.process(events, &mut l, &mut r);
        (l, r)
    }

    fn sounding(s: &June) -> usize {
        s.voices.iter().filter(|v| v.on).count()
    }

    #[test]
    fn a_chord_takes_a_voice_a_note_and_dies_away_freeing_them() {
        let mut s = synth();
        let (x, _) = render(
            &mut s,
            &[
                Event::note_on(100, 0, 48, 100, 0),
                Event::note_on(100, 0, 52, 100, 1),
                Event::note_on(100, 0, 55, 100, 2),
            ],
            4800,
        );
        assert_eq!(peak(&x[..100]), 0.0);
        assert!(peak(&x[100..]) > 0.05, "{}", peak(&x[100..]));
        assert_eq!(sounding(&s), 3);
        let (x, _) = render(
            &mut s,
            &[
                Event::note_off(0, 0, 48, 0),
                Event::note_off(0, 0, 52, 1),
                Event::note_off(0, 0, 55, 2),
            ],
            96_000,
        );
        assert_eq!(peak(&x[90_000..]), 0.0);
        assert_eq!(sounding(&s), 0);
        assert!((0..MAX_VOICES).all(|v| s.alloc.note_of(v).is_none()));
    }

    #[test]
    fn six_voices_means_six_and_the_seventh_note_steals_the_oldest() {
        let mut s = synth();
        s.params.set(at::SUSTAIN, 1.0);
        let events: Vec<Event> = (0..7)
            .map(|i| Event::note_on(i * 10, 0, 48 + i as u8, 100, i))
            .collect();
        render(&mut s, &events, 256);
        assert_eq!(s.alloc.voices(), 6);
        assert_eq!(sounding(&s), 6);
        assert_eq!(
            s.alloc.note_of(0),
            Some(54),
            "the first note's voice was taken"
        );
        // With more voices asked for, once quiet, the next chord has them.
        render(&mut s, &[Event::all_notes_off(0)], 96_000);
        s.params.set(at::VOICES, 12.0);
        render(&mut s, &events, 256);
        assert_eq!(s.alloc.voices(), 12);
        assert_eq!(sounding(&s), 7);
    }

    #[test]
    fn the_pitch_follows_the_note_and_the_range_switch() {
        let crossings = |range: f64| {
            let mut s = synth();
            s.params.set(at::RANGE, range);
            s.params.set(at::SUB, 0.0);
            s.params.set(at::CUTOFF, 20_000.0);
            s.params.set(at::FILTER_ENV, 0.0);
            s.params.set(at::SUSTAIN, 1.0);
            let (x, _) = render(&mut s, &[Event::note_on(0, 0, 69, 127, 0)], 48_000);
            x[24_000..]
                .windows(2)
                .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
                .count()
                / 2
        };
        assert!((215..=225).contains(&crossings(1.0)), "{}", crossings(1.0));
        assert!((105..=115).contains(&crossings(0.0)), "{}", crossings(0.0));
        assert!((435..=445).contains(&crossings(2.0)), "{}", crossings(2.0));
    }

    #[test]
    fn the_sub_adds_the_octave_below_and_the_highpass_switch_shapes_the_bass() {
        let spectrum = |sub: f64, highpass: f64| {
            let mut s = synth();
            s.params.set(at::SUB, sub);
            s.params.set(at::HIGHPASS, highpass);
            s.params.set(at::CUTOFF, 20_000.0);
            s.params.set(at::RESONANCE, 0.0);
            s.params.set(at::FILTER_ENV, 0.0);
            s.params.set(at::VINTAGE, 0.0);
            s.params.set(at::SUSTAIN, 1.0);
            let (x, _) = render(&mut s, &[Event::note_on(0, 0, 43, 127, 0)], 96_000);
            x[48_000..].to_vec()
        };
        let f0 = hz(43.0); // 97.999 Hz
        let plain = spectrum(0.0, 1.0);
        let with_sub = spectrum(1.0, 1.0);
        assert!(db_at(&with_sub, f0 / 2.0) > db_at(&plain, f0 / 2.0) + 20.0);
        // The lift adds bass; the cuts take it away, the higher more.
        let lift = spectrum(1.0, 0.0);
        let cut = spectrum(1.0, 2.0);
        let more = spectrum(1.0, 3.0);
        let at_sub = |x: &[f32]| db_at(x, f0 / 2.0);
        assert!(
            at_sub(&lift) > at_sub(&with_sub) + 1.5,
            "{} {}",
            at_sub(&lift),
            at_sub(&with_sub)
        );
        assert!(at_sub(&cut) < at_sub(&with_sub) - 6.0);
        assert!(at_sub(&more) < at_sub(&cut) - 6.0);
    }

    #[test]
    fn the_envelope_moves_the_filter_either_way_and_it_sings_on_its_own() {
        let bright = |fenv: f64| {
            let mut s = synth();
            s.params.set(at::CUTOFF, 800.0);
            s.params.set(at::FILTER_ENV, fenv);
            s.params.set(at::KEY_TRACK, 0.0);
            s.params.set(at::SUSTAIN, 1.0);
            s.params.set(at::VINTAGE, 0.0);
            let (x, _) = render(&mut s, &[Event::note_on(0, 0, 45, 100, 0)], 48_000);
            db_at(&x[24_000..], hz(45.0) * 16.0)
        };
        assert!(
            bright(3.0) > bright(0.0) + 20.0,
            "{} {}",
            bright(3.0),
            bright(0.0)
        );
        assert!(
            bright(-3.0) < bright(0.0) - 10.0,
            "{} {}",
            bright(-3.0),
            bright(0.0)
        );
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
        s.params.set(at::VINTAGE, 0.0);
        let (x, _) = render(&mut s, &[Event::note_on(0, 0, 60, 100, 0)], 96_000);
        let hz = crate::vintage::measure::ring_hz(&x, RATE);
        assert!((hz / 1000.0 - 1.0).abs() < 0.05, "{hz}");
        assert!(peak(&x[48_000..]) > 0.02);
    }

    #[test]
    fn the_lfo_waits_its_delay_then_moves_the_pulse_width() {
        let width_at = |delay: f64, seconds: f32| {
            let mut s = synth();
            s.params.set(at::SAW, 0.0);
            s.params.set(at::PULSE, 1.0);
            s.params.set(at::PWM_SOURCE, 1.0);
            s.params.set(at::PWM, 1.0);
            s.params.set(at::LFO_RATE, 0.5);
            s.params.set(at::LFO_DELAY, delay);
            s.params.set(at::SUSTAIN, 1.0);
            s.params.set(at::VINTAGE, 0.0);
            render(
                &mut s,
                &[Event::note_on(0, 0, 57, 100, 0)],
                (seconds * RATE) as usize,
            );
            s.voices[0].pulse.width
        };
        // Half a cycle of a 0.5 Hz triangle from its middle: at the top.
        assert!(
            (width_at(0.0, 0.5) - 0.95).abs() < 0.02,
            "{}",
            width_at(0.0, 0.5)
        );
        // With a two-second delay the LFO hasn't started by then.
        assert!(
            (width_at(2.0, 0.5) - 0.5).abs() < 0.02,
            "{}",
            width_at(2.0, 0.5)
        );
        // And by two and a half it is in, at the top of its sweep.
        assert!(width_at(2.0, 2.5) > 0.9, "{}", width_at(2.0, 2.5));
    }

    #[test]
    fn the_chorus_widens_the_sound_and_the_mono_sum_is_left_alone_without_it() {
        let sides = |chorus: f64| {
            let mut s = synth();
            s.params.set(at::CHORUS, chorus);
            s.params.set(at::SUSTAIN, 1.0);
            s.params.set(at::VINTAGE, 0.0);
            let (l, r) = render(&mut s, &[Event::note_on(0, 0, 60, 100, 0)], 48_000);
            l[24_000..]
                .iter()
                .zip(&r[24_000..])
                .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()))
        };
        assert_eq!(sides(0.0), 0.0);
        assert!(sides(1.0) > 0.02, "{}", sides(1.0));
        assert!(sides(3.0) > 0.02, "{}", sides(3.0));
    }

    #[test]
    fn unison_stacks_every_voice_on_one_note_and_mono_plays_one() {
        let mut s = synth();
        s.params.set(at::MODE, 1.0);
        s.params.set(at::SUSTAIN, 1.0);
        render(&mut s, &[Event::note_on(0, 0, 48, 100, 0)], 64);
        assert_eq!(sounding(&s), 6);
        assert!(s.voices[0].stack < 0.0 && s.voices[5].stack > 0.0);
        let mut s = synth();
        s.params.set(at::MODE, 2.0);
        s.params.set(at::SUSTAIN, 1.0);
        render(
            &mut s,
            &[
                Event::note_on(0, 0, 48, 100, 0),
                Event::note_on(10, 0, 52, 100, 1),
            ],
            64,
        );
        assert_eq!(sounding(&s), 1);
        assert_eq!(s.alloc.note_of(0), Some(52));
    }

    #[test]
    fn portamento_glides_each_voice_from_where_it_was() {
        let pitch_after = |portamento: f64| {
            let mut s = synth();
            s.params.set(at::PORTAMENTO, portamento);
            s.params.set(at::GLIDE, 1.0);
            s.params.set(at::SUSTAIN, 1.0);
            s.params.set(at::MODE, 2.0);
            render(
                &mut s,
                &[
                    Event::note_on(0, 0, 48, 100, 0),
                    Event::note_on(1000, 0, 60, 100, 1),
                ],
                1100,
            );
            s.voices[0].pitch
        };
        assert_eq!(pitch_after(0.0), 60.0);
        assert!(pitch_after(1.0) < 50.0, "{}", pitch_after(1.0));
    }

    #[test]
    fn gate_mode_holds_full_level_until_the_key_lifts() {
        let mut s = synth();
        s.params.set(at::AMP, 1.0);
        s.params.set(at::SUSTAIN, 0.0);
        s.params.set(at::DECAY, 0.01);
        s.params.set(at::FILTER_ENV, 0.0);
        s.params.set(at::CUTOFF, 5000.0);
        let (x, _) = render(&mut s, &[Event::note_on(0, 0, 45, 100, 0)], 48_000);
        assert!(peak(&x[40_000..]) > 0.03, "{}", peak(&x[40_000..]));
        let (x, _) = render(&mut s, &[Event::note_off(0, 0, 45, 0)], 4800);
        assert_eq!(peak(&x[2400..]), 0.0);
        assert_eq!(sounding(&s), 0);
    }

    #[test]
    fn velocity_and_the_wheel_do_what_they_say() {
        let level = |vel: u8| {
            let mut s = synth();
            s.params.set(at::VELOCITY, 1.0);
            s.params.set(at::SUSTAIN, 1.0);
            let (x, _) = render(&mut s, &[Event::note_on(0, 0, 60, vel, 0)], 24_000);
            peak(&x[12_000..])
        };
        assert!(level(127) > level(40) * 2.0);
        // The wheel brings in vibrato: the pitch moves with it.
        let mut s = synth();
        s.params.set(at::SUSTAIN, 1.0);
        s.params.set(at::WHEEL, 12.0);
        s.params.set(at::LFO_RATE, 0.5);
        let wheel = Event {
            frame: 0,
            channel: 0,
            kind: Kind::Control {
                number: 1,
                value: 127,
            },
        };
        render(&mut s, &[Event::note_on(0, 0, 60, 100, 0), wheel], 24_000);
        let f = s.voices[0].saw.dt * RATE;
        assert!((f - hz(60.0)).abs() > 20.0, "{f}");
    }

    /// The golden render: a fixed pad patch has a known spectrum, so a
    /// later change to the toolkit can't silently change the sound.
    #[test]
    fn the_golden_stab_renders_the_same_spectrum() {
        let mut s = synth();
        s.params.set(at::PULSE, 0.6);
        s.params.set(at::SUB, 0.4);
        s.params.set(at::CUTOFF, 500.0);
        s.params.set(at::RESONANCE, 0.25);
        s.params.set(at::FILTER_ENV, 3.0);
        s.params.set(at::DECAY, 0.25);
        s.params.set(at::SUSTAIN, 0.0);
        s.params.set(at::VINTAGE, 0.0);
        let (x, _) = render(
            &mut s,
            &[
                Event::note_on(0, 0, 48, 110, 0),
                Event::note_on(0, 0, 55, 110, 1),
            ],
            48_000,
        );
        let f0 = hz(48.0);
        let got: Vec<f32> = [0.5, 1.0, 1.5, 2.0, 3.0]
            .iter()
            .map(|h| db_at(&x[..24_000], f0 * h))
            .collect();
        for (g, w) in got.iter().zip(GOLDEN) {
            assert!((g - w).abs() < 1.0, "got {got:.1?}, want {GOLDEN:?}");
        }
    }

    /// Levels in dB at half, one, one and a half (the fifth), two and
    /// three times the fundamental of the golden stab.
    const GOLDEN: [f32; 5] = [-48.3, -59.4, -52.5, -51.7, -54.6];

    /// The plan's CPU budget: six voices at 2× oversampling with the
    /// chorus on, a few percent of one core at 48 kHz; the bound is
    /// loose because test machines differ.
    #[test]
    #[cfg_attr(debug_assertions, ignore = "timed in release builds only")]
    fn six_voices_fit_the_budget() {
        let mut s = synth();
        s.params.set(at::SUSTAIN, 1.0);
        s.params.set(at::NOISE, 0.2);
        s.params.set(at::CHORUS, 3.0);
        let seconds = 2.0;
        let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
        let chord: Vec<Event> = (0..6)
            .map(|i| Event::note_on(0, 0, 48 + 4 * i as u8, 100, i))
            .collect();
        let start = std::time::Instant::now();
        s.process(&chord, &mut l, &mut r);
        for _ in 0..(48_000.0 * seconds / 512.0) as usize {
            s.process(&[], &mut l, &mut r);
        }
        let used = start.elapsed().as_secs_f64() / seconds;
        eprintln!(
            "six voices with the chorus: {:.2}% of one core",
            used * 100.0
        );
        assert!(used < 0.2, "{:.2}% of one core", used * 100.0);
    }
}
