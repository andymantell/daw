// SPDX-License-Identifier: GPL-3.0-or-later
//! Blue Wave: the eight-bit wavetable synth with an analogue filter,
//! built the way the 1981 German classic was (`docs/plans/classic-synths.md`,
//! S9), for glassy pads, bells, digital keys and the 80s sound. Each
//! voice has two oscillators reading the same table of 64 single-cycle
//! waves ([`crate::wavetable`]): the position in the table is set by
//! hand and swept by its own envelope, the LFO, the keyboard and the
//! mod wheel, so the tone moves through the table as a note plays; the
//! second oscillator sits at its own offset in the table, transposed
//! and detuned. The oscillators run in one of two characters: smooth
//! (band-limited, interpolated, the sweep continuous) or eight-bit (the
//! waves rounded to eight bits, read without interpolation so the top
//! folds back, the sweep stepping from wave to wave), which is the
//! original's own sound kept as a switch. The two sum into the 24 dB
//! transconductance cascade low-pass with its own ADSR envelope, key
//! tracking and velocity; the amplifier has an ADSR of its own; and a
//! third ADSR moves the wave position. A shared LFO (triangle, saw,
//! square or random) with a delay gives vibrato, a filter wobble and
//! a wave wobble. The voices spread across the stereo field.
//!
//! The oscillators are digital and hold their pitch; what **Vintage
//! drift** moves is the analogue part: each voice's filter cutoff and
//! envelope speeds, a little differently per voice. Eight voices by
//! default, up to sixteen; polyphonic, stacked in unison, or one note.

use daw_engine::midi::{Event, Instrument, Kind};

use crate::dsp::{Noise, db, hz, pan};
use crate::params::{Live, Param, Unit};
use crate::vintage::{
    Action, Allocator, Analog, Cascade, Drift, Glide, GlideMode, Poles, Priority, Shape, Steps,
    VoiceSettings, soft,
};
use crate::wavetable::{self, Table};

const CHARACTERS: &[&str] = &["Smooth", "Eight-bit"];
const LFO_SHAPES: &[&str] = &["Triangle", "Saw down", "Square", "Random"];
const MODES: &[&str] = &["Poly", "Unison", "Mono"];
const PORTAMENTO: &[&str] = &["Off", "On"];

/// Settings by index; the order of [`params`].
pub mod at {
    pub const TABLE: usize = 0;
    pub const WAVE: usize = 1;
    pub const CHARACTER: usize = 2;
    pub const WAVE_ENV: usize = 3;
    pub const WAVE_LFO: usize = 4;
    pub const KEY_WAVE: usize = 5;
    pub const WHEEL_WAVE: usize = 6;
    pub const B_WAVE: usize = 7;
    pub const B_SEMI: usize = 8;
    pub const B_DETUNE: usize = 9;
    pub const MIX: usize = 10;
    pub const W_ATTACK: usize = 11;
    pub const W_DECAY: usize = 12;
    pub const W_SUSTAIN: usize = 13;
    pub const W_RELEASE: usize = 14;
    pub const CUTOFF: usize = 15;
    pub const RESONANCE: usize = 16;
    pub const FILTER_ENV: usize = 17;
    pub const KEY_TRACK: usize = 18;
    pub const FILTER_LFO: usize = 19;
    pub const VELOCITY_FILTER: usize = 20;
    pub const F_ATTACK: usize = 21;
    pub const F_DECAY: usize = 22;
    pub const F_SUSTAIN: usize = 23;
    pub const F_RELEASE: usize = 24;
    pub const ATTACK: usize = 25;
    pub const DECAY: usize = 26;
    pub const SUSTAIN: usize = 27;
    pub const RELEASE: usize = 28;
    pub const VELOCITY: usize = 29;
    pub const LFO_RATE: usize = 30;
    pub const LFO_SHAPE: usize = 31;
    pub const LFO_DELAY: usize = 32;
    pub const LFO_PITCH: usize = 33;
    pub const WHEEL: usize = 34;
    pub const MODE: usize = 35;
    pub const VOICES: usize = 36;
    pub const UNISON_DETUNE: usize = 37;
    pub const PORTAMENTO: usize = 38;
    pub const GLIDE: usize = 39;
    pub const BEND: usize = 40;
    pub const SPREAD: usize = 41;
    pub const LEVEL: usize = 42;
    pub const VINTAGE: usize = 43;
}

/// The synth's settings, in the order of [`at`]. Grouped for showing by
/// [`GROUPS`].
pub fn params() -> Vec<Param> {
    use Unit::*;
    let last = (wavetable::WAVES - 1) as f64;
    vec![
        Param::new(
            "table",
            "Wavetable",
            0.0,
            (wavetable::NAMES.len() - 1) as f64,
            0.0,
            Choice,
        )
        .choices(wavetable::NAMES),
        Param::new("wave", "Wave", 0.0, last, 0.0, Count),
        Param::new("character", "Character", 0.0, 1.0, 0.0, Choice).choices(CHARACTERS),
        Param::new("wave_env", "Envelope to wave", -last, last, 0.0, Count),
        Param::new("wave_lfo", "LFO to wave", 0.0, last, 0.0, Count),
        Param::new("key_wave", "Keyboard to wave", -24.0, 24.0, 0.0, Count),
        Param::new("wheel_wave", "Mod wheel to wave", 0.0, last, 0.0, Count),
        Param::new(
            "b_wave",
            "Oscillator B wave offset",
            -last,
            last,
            0.0,
            Count,
        ),
        Param::new(
            "b_semi",
            "Oscillator B transpose",
            -24.0,
            24.0,
            0.0,
            Semitones,
        ),
        Param::new("b_detune", "Oscillator B detune", -50.0, 50.0, 0.0, Cents),
        Param::new("mix", "Mix A to B", 0.0, 1.0, 0.5, Amount),
        Param::new("w_attack", "Wave attack", 0.0015, 10.0, 0.005, Seconds).log(),
        Param::new("w_decay", "Wave decay", 0.002, 12.0, 1.0, Seconds).log(),
        Param::new("w_sustain", "Wave sustain", 0.0, 1.0, 1.0, Amount),
        Param::new("w_release", "Wave release", 0.002, 12.0, 0.3, Seconds).log(),
        Param::new("cutoff", "Cutoff", 10.0, 20_000.0, 6_000.0, Hz).log(),
        Param::new("resonance", "Resonance", 0.0, 1.0, 0.05, Amount),
        Param::new("filter_env", "Envelope to filter", -6.0, 6.0, 0.0, Octaves),
        Param::new("key_track", "Keyboard to filter", 0.0, 1.0, 0.5, Amount),
        Param::new("filter_lfo", "LFO to filter", 0.0, 4.0, 0.0, Octaves),
        Param::new(
            "velocity_filter",
            "Velocity to filter",
            0.0,
            4.0,
            0.0,
            Octaves,
        ),
        Param::new("f_attack", "Filter attack", 0.0015, 10.0, 0.005, Seconds).log(),
        Param::new("f_decay", "Filter decay", 0.002, 12.0, 0.5, Seconds).log(),
        Param::new("f_sustain", "Filter sustain", 0.0, 1.0, 0.5, Amount),
        Param::new("f_release", "Filter release", 0.002, 12.0, 0.3, Seconds).log(),
        Param::new("attack", "Attack", 0.0015, 10.0, 0.005, Seconds).log(),
        Param::new("decay", "Decay", 0.002, 12.0, 0.5, Seconds).log(),
        Param::new("sustain", "Sustain", 0.0, 1.0, 0.8, Amount),
        Param::new("release", "Release", 0.002, 12.0, 0.3, Seconds).log(),
        Param::new("velocity", "Velocity", 0.0, 1.0, 0.4, Amount),
        Param::new("lfo_rate", "LFO rate", 0.05, 30.0, 4.0, Hz).log(),
        Param::new("lfo_shape", "LFO shape", 0.0, 3.0, 0.0, Choice).choices(LFO_SHAPES),
        Param::new("lfo_delay", "LFO delay", 0.0, 4.0, 0.0, Seconds),
        Param::new("lfo_pitch", "LFO to pitch", 0.0, 12.0, 0.0, Semitones),
        Param::new("wheel", "Mod wheel vibrato", 0.0, 12.0, 0.5, Semitones),
        Param::new("mode", "Playing", 0.0, 2.0, 0.0, Choice).choices(MODES),
        Param::new("voices", "Voices", 1.0, MAX_VOICES as f64, 8.0, Count),
        Param::new("unison_detune", "Unison detune", 0.0, 50.0, 8.0, Cents),
        Param::new("portamento", "Portamento", 0.0, 1.0, 0.0, Choice).choices(PORTAMENTO),
        Param::new("glide", "Portamento time", 0.005, 3.0, 0.1, Seconds).log(),
        Param::new("bend", "Bend range", 1.0, 24.0, 2.0, Semitones),
        Param::new("spread", "Stereo spread", 0.0, 1.0, 0.5, Amount),
        Param::new("level", "Level", -36.0, 6.0, -6.0, Db),
        Param::new("vintage", "Vintage drift", 0.0, 1.0, 0.3, Amount),
    ]
}

/// The settings in groups, for the controls: (heading, first, last).
pub const GROUPS: &[(&str, usize, usize)] = &[
    ("Wavetable", at::TABLE, at::WHEEL_WAVE),
    ("Oscillator B", at::B_WAVE, at::MIX),
    ("Wave envelope", at::W_ATTACK, at::W_RELEASE),
    ("Filter", at::CUTOFF, at::VELOCITY_FILTER),
    ("Filter envelope", at::F_ATTACK, at::F_RELEASE),
    ("Amplifier", at::ATTACK, at::VELOCITY),
    ("LFO", at::LFO_RATE, at::WHEEL),
    ("Playing", at::MODE, at::VINTAGE),
];

/// The sounds it makes from its settings.
pub fn sounds() -> Vec<crate::Sound> {
    use crate::Sound;
    vec![
        Sound {
            name: "Glass pad",
            settings: &[
                ("table", 6.0),
                ("wave", 8.0),
                ("wave_env", 30.0),
                ("wave_lfo", 4.0),
                ("w_attack", 2.0),
                ("w_decay", 3.0),
                ("w_sustain", 0.5),
                ("w_release", 1.5),
                ("b_wave", 6.0),
                ("b_detune", 7.0),
                ("b_semi", 12.0),
                ("mix", 0.4),
                ("cutoff", 5_000.0),
                ("resonance", 0.1),
                ("filter_env", 0.5),
                ("f_attack", 1.0),
                ("f_decay", 2.0),
                ("f_sustain", 0.6),
                ("f_release", 1.5),
                ("attack", 0.9),
                ("decay", 1.0),
                ("sustain", 0.8),
                ("release", 1.8),
                ("lfo_rate", 0.2),
                ("lfo_delay", 1.0),
                ("spread", 0.9),
            ],
        },
        Sound {
            name: "Wave bells",
            settings: &[
                ("table", 4.0),
                ("wave", 34.0),
                ("wave_env", -30.0),
                ("w_attack", 0.002),
                ("w_decay", 1.5),
                ("w_sustain", 0.0),
                ("w_release", 1.5),
                ("b_wave", 12.0),
                ("b_semi", 7.0),
                ("b_detune", 4.0),
                ("mix", 0.3),
                ("cutoff", 9_000.0),
                ("resonance", 0.05),
                ("filter_env", 1.5),
                ("f_decay", 0.8),
                ("f_sustain", 0.0),
                ("f_release", 1.0),
                ("attack", 0.002),
                ("decay", 2.5),
                ("sustain", 0.0),
                ("release", 2.0),
                ("velocity", 0.6),
                ("velocity_filter", 1.0),
                ("spread", 0.7),
            ],
        },
        Sound {
            name: "Resonant sweep",
            settings: &[
                ("table", 1.0),
                ("wave", 0.0),
                ("wave_env", 50.0),
                ("w_attack", 3.0),
                ("w_decay", 4.0),
                ("w_sustain", 0.3),
                ("w_release", 2.0),
                ("b_wave", 3.0),
                ("b_detune", -6.0),
                ("mix", 0.5),
                ("cutoff", 4_000.0),
                ("resonance", 0.15),
                ("filter_env", 0.0),
                ("attack", 0.6),
                ("decay", 1.0),
                ("sustain", 0.9),
                ("release", 1.5),
                ("spread", 0.8),
            ],
        },
        Sound {
            name: "Vowel pad",
            settings: &[
                ("table", 3.0),
                ("wave", 20.0),
                ("wave_lfo", 22.0),
                ("w_sustain", 1.0),
                ("b_wave", 14.0),
                ("b_detune", 5.0),
                ("mix", 0.5),
                ("cutoff", 6_000.0),
                ("resonance", 0.0),
                ("attack", 0.7),
                ("decay", 1.0),
                ("sustain", 0.9),
                ("release", 1.2),
                ("lfo_rate", 0.25),
                ("lfo_shape", 0.0),
                ("spread", 0.8),
            ],
        },
        Sound {
            name: "Eighties keys",
            settings: &[
                ("table", 2.0),
                ("wave", 30.0),
                ("wave_env", 25.0),
                ("w_attack", 0.002),
                ("w_decay", 0.6),
                ("w_sustain", 0.2),
                ("w_release", 0.3),
                ("b_wave", 10.0),
                ("b_detune", -6.0),
                ("mix", 0.5),
                ("cutoff", 3_000.0),
                ("resonance", 0.1),
                ("filter_env", 1.5),
                ("f_decay", 0.5),
                ("f_sustain", 0.3),
                ("f_release", 0.3),
                ("attack", 0.003),
                ("decay", 1.0),
                ("sustain", 0.6),
                ("release", 0.4),
                ("velocity", 0.5),
                ("velocity_filter", 1.0),
                ("spread", 0.5),
            ],
        },
        Sound {
            name: "Digital bass",
            settings: &[
                ("table", 0.0),
                ("wave", 40.0),
                ("wave_env", -30.0),
                ("w_attack", 0.002),
                ("w_decay", 0.3),
                ("w_sustain", 0.0),
                ("w_release", 0.1),
                ("b_wave", 0.0),
                ("b_semi", -12.0),
                ("mix", 0.5),
                ("cutoff", 500.0),
                ("resonance", 0.15),
                ("filter_env", 2.5),
                ("key_track", 0.4),
                ("f_decay", 0.3),
                ("f_sustain", 0.0),
                ("f_release", 0.1),
                ("attack", 0.002),
                ("decay", 0.4),
                ("sustain", 0.4),
                ("release", 0.1),
                ("velocity", 0.4),
                ("velocity_filter", 0.8),
                ("mode", 2.0),
                ("spread", 0.0),
            ],
        },
        Sound {
            name: "Sync lead",
            settings: &[
                ("table", 7.0),
                ("wave", 6.0),
                ("wave_env", 40.0),
                ("w_attack", 0.01),
                ("w_decay", 0.5),
                ("w_sustain", 0.25),
                ("w_release", 0.3),
                ("b_wave", 0.0),
                ("b_detune", 5.0),
                ("mix", 0.3),
                ("cutoff", 6_000.0),
                ("resonance", 0.1),
                ("filter_env", 1.0),
                ("f_decay", 0.4),
                ("f_sustain", 0.5),
                ("attack", 0.005),
                ("decay", 0.4),
                ("sustain", 0.7),
                ("release", 0.3),
                ("mode", 2.0),
                ("portamento", 1.0),
                ("glide", 0.06),
                ("lfo_delay", 0.5),
                ("lfo_rate", 5.5),
                ("lfo_pitch", 0.15),
                ("wheel", 1.0),
                ("spread", 0.0),
            ],
        },
        Sound {
            name: "Folded lead",
            settings: &[
                ("table", 9.0),
                ("wave", 10.0),
                ("wave_env", 35.0),
                ("w_attack", 0.08),
                ("w_decay", 1.0),
                ("w_sustain", 0.4),
                ("w_release", 0.4),
                ("b_wave", -4.0),
                ("b_detune", 8.0),
                ("mix", 0.5),
                ("cutoff", 5_000.0),
                ("resonance", 0.15),
                ("filter_env", 0.5),
                ("attack", 0.01),
                ("decay", 0.5),
                ("sustain", 0.8),
                ("release", 0.4),
                ("mode", 1.0),
                ("voices", 4.0),
                ("unison_detune", 10.0),
                ("portamento", 1.0),
                ("glide", 0.05),
                ("lfo_delay", 0.6),
                ("lfo_pitch", 0.2),
                ("spread", 0.6),
            ],
        },
        Sound {
            name: "Organ",
            settings: &[
                ("table", 5.0),
                ("wave", 48.0),
                ("wave_env", 0.0),
                ("wheel_wave", 11.0),
                ("b_wave", 0.0),
                ("b_semi", 12.0),
                ("b_detune", 3.0),
                ("mix", 0.25),
                ("cutoff", 9_000.0),
                ("resonance", 0.0),
                ("filter_env", 0.0),
                ("attack", 0.003),
                ("decay", 0.1),
                ("sustain", 1.0),
                ("release", 0.05),
                ("velocity", 0.0),
                ("lfo_rate", 6.0),
                ("lfo_pitch", 0.0),
                ("spread", 0.3),
            ],
        },
        Sound {
            name: "Eight-bit stab",
            settings: &[
                ("table", 0.0),
                ("wave", 59.0),
                ("character", 1.0),
                ("wave_env", -45.0),
                ("w_attack", 0.002),
                ("w_decay", 0.25),
                ("w_sustain", 0.0),
                ("w_release", 0.2),
                ("b_wave", -20.0),
                ("b_semi", -12.0),
                ("b_detune", 6.0),
                ("mix", 0.4),
                ("cutoff", 2_500.0),
                ("resonance", 0.2),
                ("filter_env", 2.0),
                ("f_decay", 0.3),
                ("f_sustain", 0.0),
                ("f_release", 0.2),
                ("attack", 0.002),
                ("decay", 0.35),
                ("sustain", 0.0),
                ("release", 0.25),
                ("velocity", 0.5),
                ("velocity_filter", 1.0),
                ("spread", 0.4),
            ],
        },
    ]
}

/// The most voices there are.
pub const MAX_VOICES: usize = 16;

/// Settings are read, and the filters set, this often: a third of a
/// millisecond.
const CHUNK: usize = 16;

/// One voice.
struct Voice {
    /// The oscillators' phases, 0 to 1.
    phase_a: f32,
    phase_b: f32,
    filter: Cascade,
    env: Analog,
    filter_env: Analog,
    wave_env: Analog,
    drift: Drift,
    on: bool,
    /// The pitch now, gliding towards `target`.
    pitch: f32,
    target: f32,
    velocity: f32,
    /// Where in a unison stack it sits, −1 to 1, for its detune.
    stack: f32,
    /// Its place across the stereo field, −1 to 1, before the spread.
    place: f32,
    /// The wave position it is reading, for the tests.
    position: f32,
}

impl Voice {
    fn new(seed: u32, place: f32) -> Voice {
        Voice {
            phase_a: 0.0,
            phase_b: 0.0,
            filter: Cascade::new(),
            env: Analog::IDLE,
            filter_env: Analog::IDLE,
            wave_env: Analog::IDLE,
            drift: Drift::new(seed),
            on: false,
            pitch: 60.0,
            target: 60.0,
            velocity: 1.0,
            stack: 0.0,
            place,
            position: 0.0,
        }
    }
}

/// The synth on the audio thread.
pub struct BlueWave {
    params: Live,
    rate: f32,
    alloc: Allocator,
    voices: Vec<Voice>,
    noise: Noise,
    bend: f32,
    wheel: f32,
    /// Keys down, to know when the LFO's delay restarts.
    keys_down: u32,
    lfo_phase: f32,
    /// The random shape's held value, and the fade after the delay.
    lfo_hold: f32,
    lfo_fade: f32,
    lfo_since: f32,
    actions: Vec<Action>,
    /// The chunk's stereo mix.
    mix_l: [f32; CHUNK],
    mix_r: [f32; CHUNK],
}

impl BlueWave {
    pub fn new(rate: u32, params: impl Into<Live>) -> BlueWave {
        BlueWave {
            params: params.into(),
            rate: rate.max(1) as f32,
            alloc: Allocator::new(MAX_VOICES),
            voices: (0..MAX_VOICES as u32)
                .map(|i| {
                    // Voices sit left and right in turn, widening out.
                    let place = [0.0, 0.5, -0.5, 1.0, -1.0, 0.25, -0.25, 0.75][i as usize % 8]
                        * if i >= 8 { -1.0 } else { 1.0 };
                    Voice::new(i + 1, place)
                })
                .collect(),
            noise: Noise::new(0x1981_0b1e),
            bend: 0.0,
            wheel: 0.0,
            keys_down: 0,
            lfo_phase: 0.0,
            lfo_hold: 0.0,
            lfo_fade: 1.0,
            lfo_since: 0.0,
            actions: Vec::with_capacity(2 * MAX_VOICES),
            mix_l: [0.0; CHUNK],
            mix_r: [0.0; CHUNK],
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

    /// The allocator is sized to the setting whenever every voice is
    /// quiet, so an eight-voice synth has eight voices and no more.
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
                        // The oscillators start from the top of their
                        // cycle, as a digital oscillator does; the
                        // filter starts clean.
                        v.phase_a = 0.0;
                        v.phase_b = 0.0;
                        v.filter.reset();
                    }
                    v.stack = if s.unison > 1 {
                        voice as f32 / (s.unison - 1) as f32 * 2.0 - 1.0
                    } else {
                        0.0
                    };
                    v.velocity = 1.0 - sens + sens * velocity.clamp(1, 127) as f32 / 127.0;
                    v.env.trigger();
                    v.filter_env.trigger();
                    v.wave_env.trigger();
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
                    let v = &mut self.voices[voice];
                    v.env.release();
                    v.filter_env.release();
                    v.wave_env.release();
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
                    self.lfo_phase = 0.0;
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

    /// The LFO's value at `phase` for `shape`, −1 to 1; the random
    /// shape holds a value a cycle.
    #[inline]
    fn lfo(shape: u32, phase: f32, hold: f32) -> f32 {
        match shape {
            1 => 1.0 - 2.0 * phase,
            2 => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            3 => hold,
            _ => {
                // A triangle from 0, rising first.
                let p = (phase + 0.25).fract();
                1.0 - 4.0 * (p - 0.5).abs()
            }
        }
    }

    fn chunk(&mut self, from: usize, to: usize, left: &mut [f32], right: &mut [f32]) {
        let p = &self.params;
        let r = self.rate;
        let count = to - from;
        let n = count as f32;

        // The LFO, shared by the voices, with its delay: it fades in
        // over a quarter of the delay once the delay has passed.
        let lfo_shape = p.read(at::LFO_SHAPE) as u32;
        let lfo_dt = p.read(at::LFO_RATE) / r;
        let delay = p.read(at::LFO_DELAY);
        self.lfo_since += n / r;
        self.lfo_fade = if delay <= 0.0 {
            1.0
        } else {
            ((self.lfo_since - delay) / (0.25 * delay)).clamp(0.0, 1.0)
        };
        let next_phase = self.lfo_phase + lfo_dt * n;
        if next_phase >= 1.0 {
            self.lfo_hold = self.noise.tick();
        }
        let lfo_now = BlueWave::lfo(lfo_shape, self.lfo_phase, self.lfo_hold) * self.lfo_fade;
        self.lfo_phase = next_phase.fract();
        if self.voices.iter().all(|v| !v.on) {
            return;
        }

        let table = Table::get(p.read(at::TABLE) as usize);
        let wave = p.read(at::WAVE);
        let rough = p.read(at::CHARACTER) >= 0.5;
        let wave_env = p.read(at::WAVE_ENV);
        let wave_lfo = p.read(at::WAVE_LFO);
        let key_wave = p.read(at::KEY_WAVE);
        let wheel_wave = p.read(at::WHEEL_WAVE) * self.wheel;
        let b_wave = p.read(at::B_WAVE);
        let b_semi = p.read(at::B_SEMI) + p.read(at::B_DETUNE) / 100.0;
        let mix = p.read(at::MIX);
        let cutoff = p.read(at::CUTOFF);
        let res = p.read(at::RESONANCE);
        let fenv = p.read(at::FILTER_ENV);
        let track = p.read(at::KEY_TRACK);
        let flfo = p.read(at::FILTER_LFO);
        let vel_filter = p.read(at::VELOCITY_FILTER);
        let shapes = [
            Shape::adsr(
                p.read(at::ATTACK),
                p.read(at::DECAY),
                p.read(at::SUSTAIN),
                p.read(at::RELEASE),
            ),
            Shape::adsr(
                p.read(at::F_ATTACK),
                p.read(at::F_DECAY),
                p.read(at::F_SUSTAIN),
                p.read(at::F_RELEASE),
            ),
            Shape::adsr(
                p.read(at::W_ATTACK),
                p.read(at::W_DECAY),
                p.read(at::W_SUSTAIN),
                p.read(at::W_RELEASE),
            ),
        ];
        let glide = Glide {
            mode: GlideMode::Time,
            seconds: p.read(at::GLIDE),
        };
        let detune = p.read(at::UNISON_DETUNE) / 100.0;
        let bend = self.bend * p.read(at::BEND);
        let lfo_pitch = (p.read(at::LFO_PITCH) + self.wheel * p.read(at::WHEEL)) * lfo_now;
        let spread = p.read(at::SPREAD);
        let gain = db(p.read(at::LEVEL));
        let vintage = p.read(at::VINTAGE);
        // Two oscillators sum into the filter at a level that leaves
        // room, so a clean wave stays clean through its input stage;
        // several voices of a chord sum after it.
        let mix_gain = 0.3;
        let voice_gain = 1.0;

        self.mix_l[..count].iter_mut().for_each(|v| *v = 0.0);
        self.mix_r[..count].iter_mut().for_each(|v| *v = 0.0);

        for (vi, v) in self.voices.iter_mut().enumerate() {
            if !v.on {
                continue;
            }
            // Once a chunk: the pitch with its glide, the drift of this
            // voice's analogue parts, the envelopes' speeds, the
            // oscillators' rates and copies, and the filter's cutoff.
            v.pitch = glide.step(v.pitch, v.target, n, r);
            v.drift.next(vintage, n, r);
            let scatter = vintage * v.drift.offset();
            let speed = 1.0 + 0.08 * scatter;
            let steps = shapes.map(|s| {
                Steps::new(
                    Shape {
                        attack: s.attack * speed,
                        decay: s.decay * speed,
                        release: s.release * speed,
                        ..s
                    },
                    r,
                )
            });
            let pitch = v.pitch + bend + lfo_pitch + v.stack * detune;
            let dt_a = (hz(pitch) / r).min(0.5);
            let dt_b = (hz(pitch + b_semi) / r).min(0.5);
            let mip_a = Table::mip_for(dt_a);
            let mip_b = Table::mip_for(dt_b);
            let octaves = track * (pitch - 60.0) / 12.0
                + fenv * v.filter_env.level
                + flfo * lfo_now
                + vel_filter * (v.velocity - 1.0)
                + 0.06 * scatter;
            v.filter
                .set(Poles::Four, cutoff * octaves.exp2(), res, 1.0, 0.3, r);
            // The wave position moves at chunk rate too: the envelope's
            // step across a third of a millisecond is well under a wave.
            let position = wave
                + wave_env * v.wave_env.level
                + wave_lfo * 0.5 * (lfo_now + 1.0)
                + key_wave * (v.pitch - 60.0) / 12.0
                + wheel_wave;
            let last = (wavetable::WAVES - 1) as f32;
            let pos_a = position.clamp(0.0, last);
            let pos_b = (position + b_wave).clamp(0.0, last);
            v.position = pos_a;
            let level = v.velocity * gain * voice_gain;
            let place = spread * if v.stack != 0.0 { v.stack } else { v.place };
            let (pl, pr) = pan(place);
            let mut done = false;
            for i in 0..count {
                let e = v.env.next(&steps[0]);
                v.filter_env.next(&steps[1]);
                v.wave_env.next(&steps[2]);
                let (a, b) = if rough {
                    (table.rough(pos_a, v.phase_a), table.rough(pos_b, v.phase_b))
                } else {
                    (
                        table.smooth(pos_a, mip_a, v.phase_a),
                        table.smooth(pos_b, mip_b, v.phase_b),
                    )
                };
                v.phase_a += dt_a;
                if v.phase_a >= 1.0 {
                    v.phase_a -= 1.0;
                }
                v.phase_b += dt_b;
                if v.phase_b >= 1.0 {
                    v.phase_b -= 1.0;
                }
                let x = ((1.0 - mix) * a + mix * b) * mix_gain;
                let y = v.filter.run(x) * e * level;
                self.mix_l[i] += y * pl;
                self.mix_r[i] += y * pr;
                if !v.env.active() {
                    done = true;
                    break;
                }
            }
            if done {
                v.on = false;
                self.alloc.free(vi);
            }
        }

        // The voices' sum, limited softly.
        for i in 0..count {
            left[from + i] += soft(self.mix_l[i] * 1.5) / 1.5;
            right[from + i] += soft(self.mix_r[i] * 1.5) / 1.5;
        }
    }
}

impl Instrument for BlueWave {
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

    fn synth() -> BlueWave {
        BlueWave::new(RATE as u32, Arc::new(Params::new(params())))
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

    fn render(s: &mut BlueWave, events: &[Event], n: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        s.process(events, &mut l, &mut r);
        (l, r)
    }

    fn sounding(s: &BlueWave) -> usize {
        s.voices.iter().filter(|v| v.on).count()
    }

    /// A synth with nothing moving: one oscillator, the filter open,
    /// no drift, the envelopes flat.
    fn plain() -> BlueWave {
        let s = synth();
        s.params.set(at::MIX, 0.0);
        s.params.set(at::CUTOFF, 20_000.0);
        s.params.set(at::RESONANCE, 0.0);
        s.params.set(at::KEY_TRACK, 0.0);
        s.params.set(at::SUSTAIN, 1.0);
        s.params.set(at::VINTAGE, 0.0);
        s.params.set(at::SPREAD, 0.0);
        // Quiet enough that the output's soft limit adds nothing.
        s.params.set(at::LEVEL, -20.0);
        s
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
    fn eight_voices_means_eight_and_the_ninth_note_steals_the_oldest() {
        let mut s = synth();
        let events: Vec<Event> = (0..9)
            .map(|i| Event::note_on(i * 10, 0, 48 + i as u8, 100, i))
            .collect();
        render(&mut s, &events, 256);
        assert_eq!(s.alloc.voices(), 8);
        assert_eq!(sounding(&s), 8);
        assert_eq!(s.alloc.note_of(0), Some(56));
        render(&mut s, &[Event::all_notes_off(0)], 96_000);
        s.params.set(at::VOICES, 12.0);
        render(&mut s, &events, 256);
        assert_eq!(s.alloc.voices(), 12);
        assert_eq!(sounding(&s), 9);
    }

    #[test]
    fn the_pitch_follows_the_note_and_oscillator_b_its_transpose() {
        let crossings = |mix: f64, semi: f64| {
            let mut s = plain();
            s.params.set(at::WAVE, 0.0);
            s.params.set(at::MIX, mix);
            s.params.set(at::B_SEMI, semi);
            let (x, _) = render(&mut s, &[Event::note_on(0, 0, 69, 127, 0)], 48_000);
            x[24_000..]
                .windows(2)
                .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
                .count()
                / 2
        };
        // Table 0's first wave is a sine: 440 Hz crosses zero 440 times
        // a second in each direction.
        assert!(
            (215..=225).contains(&crossings(0.0, 0.0)),
            "{}",
            crossings(0.0, 0.0)
        );
        assert!(
            (435..=445).contains(&crossings(1.0, 12.0)),
            "{}",
            crossings(1.0, 12.0)
        );
    }

    #[test]
    fn the_wave_position_sweeps_the_tone_from_a_sine_to_a_saw() {
        let harmonic = |wave: f64, n: f32| {
            let mut s = plain();
            s.params.set(at::WAVE, wave);
            let (x, _) = render(&mut s, &[Event::note_on(0, 0, 57, 127, 0)], 48_000);
            db_at(&x[24_000..], hz(57.0) * n)
        };
        // The sine has no third harmonic; the full saw at 63 has it,
        // and the table's own last wave in between.
        assert!(
            harmonic(0.0, 3.0) < harmonic(0.0, 1.0) - 35.0,
            "{} {}",
            harmonic(0.0, 3.0),
            harmonic(0.0, 1.0)
        );
        assert!(
            harmonic(59.0, 3.0) > harmonic(0.0, 3.0) + 20.0,
            "{} {}",
            harmonic(59.0, 3.0),
            harmonic(0.0, 3.0)
        );
        assert!((harmonic(63.0, 1.0) - harmonic(63.0, 3.0) - 9.5).abs() < 1.0);
        // The square at 62 has no second harmonic.
        assert!(harmonic(62.0, 2.0) < harmonic(62.0, 1.0) - 30.0);
    }

    #[test]
    fn the_wave_envelope_the_keyboard_and_the_lfo_move_the_position() {
        let position = |env: f64, key: f64, lfo: f64, note: u8, seconds: f32| {
            let mut s = plain();
            s.params.set(at::WAVE, 10.0);
            s.params.set(at::WAVE_ENV, env);
            s.params.set(at::W_ATTACK, 0.002);
            s.params.set(at::W_DECAY, 0.5);
            s.params.set(at::W_SUSTAIN, 0.0);
            s.params.set(at::KEY_WAVE, key);
            s.params.set(at::WAVE_LFO, lfo);
            s.params.set(at::LFO_RATE, 1.0);
            render(
                &mut s,
                &[Event::note_on(0, 0, note, 100, 0)],
                (seconds * RATE) as usize,
            );
            s.voices[0].position
        };
        // The envelope lifts the position by its full amount at the
        // peak and lets it back down.
        assert!((position(30.0, 0.0, 0.0, 60, 0.004) - 40.0).abs() < 2.0);
        assert!((position(30.0, 0.0, 0.0, 60, 3.0) - 10.0).abs() < 0.5);
        // The keyboard: twelve waves an octave, either way.
        assert!((position(0.0, 12.0, 0.0, 72, 0.1) - 22.0).abs() < 0.1);
        assert_eq!(position(0.0, 12.0, 0.0, 48, 0.1), 0.0);
        // The LFO: a quarter cycle of a 1 Hz triangle later it is at
        // the top of its swing.
        assert!((position(0.0, 0.0, 20.0, 60, 0.25) - 30.0).abs() < 1.0);
        // And it never leaves the table.
        assert!(position(100.0, 0.0, 0.0, 60, 0.004) <= 63.0);
    }

    #[test]
    fn eight_bit_keeps_the_aliasing_and_the_rounding_that_smooth_leaves_out() {
        let far = |character: f64| {
            let mut s = plain();
            s.params.set(at::WAVE, 59.0);
            s.params.set(at::CHARACTER, character);
            // High up, where the full table's harmonics fold back.
            let (x, _) = render(&mut s, &[Event::note_on(0, 0, 96, 127, 0)], 48_000);
            let x = &x[24_000..];
            // Where the saw's harmonics above the eleventh land once
            // they fold back: the loudest of them.
            let f0 = hz(96.0);
            (12..40)
                .map(|n| {
                    let f = (n as f32 * f0) % RATE;
                    if f > RATE / 2.0 { RATE - f } else { f }
                })
                .map(|f| db_at(x, f))
                .fold(-200.0f32, f32::max)
        };
        assert!(far(1.0) > far(0.0) + 20.0, "{} {}", far(1.0), far(0.0));
        // Eight-bit steps between waves rather than blending.
        let mut s = plain();
        s.params.set(at::CHARACTER, 1.0);
        s.params.set(at::WAVE, 0.0);
        s.params.set(at::KEY_WAVE, 0.3);
        render(&mut s, &[Event::note_on(0, 0, 61, 100, 0)], 256);
        let table = Table::get(0);
        let rough = table.rough(s.voices[0].position, 0.3);
        assert_eq!(rough, table.rough(0.0, 0.3));
    }

    #[test]
    fn the_filter_envelope_opens_the_filter_and_it_sings_on_its_own() {
        let bright = |fenv: f64| {
            let mut s = plain();
            s.params.set(at::WAVE, 63.0);
            s.params.set(at::CUTOFF, 600.0);
            s.params.set(at::FILTER_ENV, fenv);
            s.params.set(at::F_SUSTAIN, 1.0);
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
        // A sine far above the cutoff, resonance full: the filter
        // whistles at its cutoff and the sine is 70 dB under it.
        let mut s = plain();
        s.params.set(at::WAVE, 0.0);
        s.params.set(at::LEVEL, 0.0);
        s.params.set(at::CUTOFF, 1000.0);
        s.params.set(at::RESONANCE, 1.0);
        let (x, _) = render(&mut s, &[Event::note_on(0, 0, 120, 100, 0)], 96_000);
        let ring = crate::vintage::measure::ring_hz(&x, RATE);
        assert!((ring / 1000.0 - 1.0).abs() < 0.06, "{ring}");
    }

    #[test]
    fn velocity_the_wheel_and_the_spread_do_what_they_say() {
        let level = |vel: u8| {
            let mut s = plain();
            s.params.set(at::VELOCITY, 1.0);
            let (x, _) = render(&mut s, &[Event::note_on(0, 0, 60, vel, 0)], 24_000);
            peak(&x[12_000..])
        };
        assert!(level(127) > level(40) * 2.0);
        // The wheel moves the wave: with it up, the sine wave gains a
        // third harmonic.
        let third = |wheel: u8| {
            let mut s = plain();
            s.params.set(at::WHEEL_WAVE, 59.0);
            let w = Event {
                frame: 0,
                channel: 0,
                kind: Kind::Control {
                    number: 1,
                    value: wheel,
                },
            };
            let (x, _) = render(&mut s, &[w, Event::note_on(0, 0, 57, 100, 0)], 48_000);
            db_at(&x[24_000..], hz(57.0) * 3.0)
        };
        assert!(third(127) > third(0) + 20.0, "{} {}", third(127), third(0));
        // The spread puts a chord's voices left and right.
        let sides = |spread: f64| {
            let mut s = synth();
            s.params.set(at::SPREAD, spread);
            s.params.set(at::VINTAGE, 0.0);
            let (l, r) = render(
                &mut s,
                &[
                    Event::note_on(0, 0, 60, 100, 0),
                    Event::note_on(0, 0, 64, 100, 1),
                ],
                24_000,
            );
            l[12_000..]
                .iter()
                .zip(&r[12_000..])
                .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()))
        };
        assert_eq!(sides(0.0), 0.0);
        assert!(sides(1.0) > 0.02, "{}", sides(1.0));
    }

    #[test]
    fn unison_stacks_every_voice_on_one_note_mono_plays_one_and_glides() {
        let mut s = synth();
        s.params.set(at::MODE, 1.0);
        render(&mut s, &[Event::note_on(0, 0, 48, 100, 0)], 64);
        assert_eq!(sounding(&s), 8);
        assert!(s.voices[0].stack < 0.0 && s.voices[7].stack > 0.0);
        let pitch_after = |portamento: f64| {
            let mut s = synth();
            s.params.set(at::MODE, 2.0);
            s.params.set(at::PORTAMENTO, portamento);
            s.params.set(at::GLIDE, 1.0);
            render(
                &mut s,
                &[
                    Event::note_on(0, 0, 48, 100, 0),
                    Event::note_on(1000, 0, 60, 100, 1),
                ],
                1100,
            );
            assert_eq!(sounding(&s), 1);
            s.voices[0].pitch
        };
        assert_eq!(pitch_after(0.0), 60.0);
        assert!(pitch_after(1.0) < 50.0, "{}", pitch_after(1.0));
    }

    #[test]
    fn every_sound_made_in_code_names_real_settings_and_plays() {
        let keys: Vec<String> = params().into_iter().map(|p| p.key).collect();
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
            let mut s = BlueWave::new(RATE as u32, p);
            let (x, _) = render(&mut s, &[Event::note_on(0, 0, 60, 100, 0)], 48_000);
            assert!(peak(&x) > 0.02, "{} is silent", sound.name);
            assert!(peak(&x) <= 1.0, "{} clips", sound.name);
        }
    }

    /// The golden render: the bells at C4 have a known spectrum, so a
    /// later change to the tables or the toolkit can't silently change
    /// the sound.
    #[test]
    fn the_golden_bells_render_the_same_spectrum() {
        let p = Arc::new(Params::new(params()));
        let keys: Vec<String> = params().into_iter().map(|p| p.key).collect();
        let bells = sounds()
            .into_iter()
            .find(|s| s.name == "Wave bells")
            .unwrap();
        for (k, v) in bells.settings {
            p.set(keys.iter().position(|key| key == k).unwrap(), *v);
        }
        p.set(at::VINTAGE, 0.0);
        p.set(at::SPREAD, 0.0);
        let mut s = BlueWave::new(RATE as u32, p);
        let (x, _) = render(&mut s, &[Event::note_on(0, 0, 60, 110, 0)], 48_000);
        let f0 = hz(60.0);
        let got: Vec<f32> = [1.0, 1.5, 3.0, 5.0, 7.0]
            .iter()
            .map(|h| db_at(&x[..24_000], f0 * h))
            .collect();
        for (g, w) in got.iter().zip(GOLDEN) {
            assert!((g - w).abs() < 1.0, "got {got:.1?}, want {GOLDEN:?}");
        }
    }

    /// Levels in dB at one, one and a half (oscillator B's fifth),
    /// three, five and seven times the fundamental of the golden bells.
    const GOLDEN: [f32; 5] = [-41.2, -51.3, -40.6, -39.3, -40.0];

    /// The plan's CPU budget: sixteen voices with the filter at 2×
    /// oversampling, a few percent of one core at 48 kHz; the bound is
    /// loose because test machines differ.
    #[test]
    #[cfg_attr(debug_assertions, ignore = "timed in release builds only")]
    fn sixteen_voices_fit_the_budget() {
        let mut s = synth();
        s.params.set(at::VOICES, 16.0);
        s.params.set(at::SUSTAIN, 1.0);
        s.params.set(at::WAVE_LFO, 10.0);
        let seconds = 2.0;
        let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
        let chord: Vec<Event> = (0..16)
            .map(|i| Event::note_on(0, 0, 36 + 3 * i as u8, 100, i))
            .collect();
        let start = std::time::Instant::now();
        s.process(&chord, &mut l, &mut r);
        for _ in 0..(48_000.0 * seconds / 512.0) as usize {
            s.process(&[], &mut l, &mut r);
        }
        let used = start.elapsed().as_secs_f64() / seconds;
        eprintln!("sixteen voices: {:.2}% of one core", used * 100.0);
        assert!(used < 0.25, "{:.2}% of one core", used * 100.0);
    }
}
