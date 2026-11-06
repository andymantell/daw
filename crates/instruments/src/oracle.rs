// SPDX-License-Identifier: GPL-3.0-or-later
//! Oracle: a two-oscillator analogue polysynth with two filter boards,
//! built the way the late-1970s American poly classics were
//! (`docs/plans/classic-synths.md`, S5), for brass, pads, leads and
//! bass. Each voice has two oscillators whose saw, triangle and pulse
//! shapes can all sound at once, pulse-width modulation, oscillator A
//! hard-synced to B, oscillator B free of the keyboard or slowed to a
//! modulator; poly-mod, where the filter envelope and oscillator B
//! modulate oscillator A's pitch and pulse width and the filter cutoff
//! in every voice; white noise; a choice of two filter boards, the
//! four-pole transconductance cascade or the two-pole state-variable
//! filter with low, notch, high and band outputs; two ADSR envelopes
//! with velocity; an LFO whose saw, triangle and square can all be on at
//! once, to pitch, pulse width and filter from an initial amount and
//! the wheel; unison stacking with detune and spread; glide in every
//! voice; and a vintage amount that drifts the oscillators and scatters
//! the filters a little from voice to voice.
//!
//! Everything nonlinear is where the circuit has it: the filters'
//! saturating loops (oversampled) and the amplifier's soft limit. One
//! output on both sides, apart from the spread of a unison stack.

use daw_engine::midi::{Event, Instrument, Kind};

use crate::dsp::{Noise, db, hz, pan};
use crate::params::{Live, Param, Unit};
use crate::vintage::{
    Action, Allocator, Analog, Cascade, Drift, Glide, GlideMode, Osc, Poles, Priority, Shape,
    Steps, Svf, SvfOut, VoiceSettings, Wave, soft,
};

/// Voices in the box.
pub const VOICES: usize = 16;
/// Most voices on one note.
const UNISON: usize = 8;

const BOARDS: &[&str] = &["A: 4-pole cascade", "B: 2-pole state-variable"];
const MODES: &[&str] = &["Low", "Notch", "High", "Band"];
const KEY_TRACK: &[&str] = &["Off", "Half", "Full"];

/// Settings by index; the order of [`params`].
pub mod at {
    pub const A_SAW: usize = 0;
    pub const A_TRI: usize = 1;
    pub const A_PULSE: usize = 2;
    pub const A_WIDTH: usize = 3;
    pub const A_SYNC: usize = 4;
    pub const B_SAW: usize = 5;
    pub const B_TRI: usize = 6;
    pub const B_PULSE: usize = 7;
    pub const B_WIDTH: usize = 8;
    pub const B_COARSE: usize = 9;
    pub const B_FINE: usize = 10;
    pub const B_LOW: usize = 11;
    pub const B_KEYS: usize = 12;
    pub const A_LEVEL: usize = 13;
    pub const B_LEVEL: usize = 14;
    pub const NOISE_LEVEL: usize = 15;
    pub const PM_ENV: usize = 16;
    pub const PM_OSC_B: usize = 17;
    pub const PM_FREQ: usize = 18;
    pub const PM_WIDTH: usize = 19;
    pub const PM_FILTER: usize = 20;
    pub const BOARD: usize = 21;
    pub const MODE: usize = 22;
    pub const CUTOFF: usize = 23;
    pub const RESONANCE: usize = 24;
    pub const ENV_AMOUNT: usize = 25;
    pub const KEY_TRACK: usize = 26;
    pub const F_ATTACK: usize = 27;
    pub const F_DECAY: usize = 28;
    pub const F_SUSTAIN: usize = 29;
    pub const F_RELEASE: usize = 30;
    pub const VEL_FILTER: usize = 31;
    pub const ATTACK: usize = 32;
    pub const DECAY: usize = 33;
    pub const SUSTAIN: usize = 34;
    pub const RELEASE: usize = 35;
    pub const VEL_AMP: usize = 36;
    pub const LFO_RATE: usize = 37;
    pub const LFO_SAW: usize = 38;
    pub const LFO_TRI: usize = 39;
    pub const LFO_SQUARE: usize = 40;
    pub const LFO_INITIAL: usize = 41;
    pub const LFO_FREQ_A: usize = 42;
    pub const LFO_FREQ_B: usize = 43;
    pub const LFO_WIDTH_A: usize = 44;
    pub const LFO_WIDTH_B: usize = 45;
    pub const LFO_FILTER: usize = 46;
    pub const VOICES: usize = 47;
    pub const UNISON: usize = 48;
    pub const DETUNE: usize = 49;
    pub const SPREAD: usize = 50;
    pub const GLIDE: usize = 51;
    pub const LEGATO: usize = 52;
    pub const BEND: usize = 53;
    pub const LEVEL: usize = 54;
    pub const VINTAGE: usize = 55;
}

/// The synth's settings, in the order of [`at`]. Grouped for showing by
/// [`GROUPS`].
pub fn params() -> Vec<Param> {
    use Unit::*;
    vec![
        Param::new("a_saw", "Osc A saw", 0.0, 1.0, 1.0, Switch),
        Param::new("a_tri", "Osc A triangle", 0.0, 1.0, 0.0, Switch),
        Param::new("a_pulse", "Osc A pulse", 0.0, 1.0, 0.0, Switch),
        Param::new("a_width", "Osc A pulse width", 0.05, 0.95, 0.5, Amount),
        Param::new("a_sync", "Osc A synced to B", 0.0, 1.0, 0.0, Switch),
        Param::new("b_saw", "Osc B saw", 0.0, 1.0, 1.0, Switch),
        Param::new("b_tri", "Osc B triangle", 0.0, 1.0, 0.0, Switch),
        Param::new("b_pulse", "Osc B pulse", 0.0, 1.0, 0.0, Switch),
        Param::new("b_width", "Osc B pulse width", 0.05, 0.95, 0.5, Amount),
        Param::new("b_coarse", "Osc B frequency", -24.0, 24.0, 0.0, Semitones),
        Param::new("b_fine", "Osc B fine", -1.0, 1.0, 0.06, Semitones),
        Param::new("b_low", "Osc B low frequency", 0.0, 1.0, 0.0, Switch),
        Param::new("b_keys", "Osc B follows the keys", 0.0, 1.0, 1.0, Switch),
        Param::new("a_level", "Osc A level", 0.0, 1.0, 0.8, Amount),
        Param::new("b_level", "Osc B level", 0.0, 1.0, 0.6, Amount),
        Param::new("noise_level", "Noise level", 0.0, 1.0, 0.0, Amount),
        Param::new("pm_env", "Poly-mod: filter envelope", 0.0, 1.0, 0.0, Amount),
        Param::new("pm_osc_b", "Poly-mod: osc B", 0.0, 1.0, 0.0, Amount),
        Param::new("pm_freq", "Poly-mod to osc A pitch", 0.0, 1.0, 0.0, Switch),
        Param::new("pm_width", "Poly-mod to osc A width", 0.0, 1.0, 0.0, Switch),
        Param::new("pm_filter", "Poly-mod to filter", 0.0, 1.0, 0.0, Switch),
        Param::new("board", "Filter board", 0.0, 1.0, 0.0, Choice).choices(BOARDS),
        Param::new("mode", "Board B output", 0.0, 3.0, 0.0, Choice).choices(MODES),
        Param::new("cutoff", "Cutoff", 20.0, 18_000.0, 1200.0, Hz).log(),
        Param::new("resonance", "Resonance", 0.0, 1.0, 0.2, Amount),
        Param::new("env_amount", "Envelope amount", 0.0, 8.0, 3.0, Octaves),
        Param::new("key_track", "Keyboard tracking", 0.0, 2.0, 1.0, Choice).choices(KEY_TRACK),
        Param::new("f_attack", "Filter attack", 0.001, 10.0, 0.005, Seconds).log(),
        Param::new("f_decay", "Filter decay", 0.004, 10.0, 0.5, Seconds).log(),
        Param::new("f_sustain", "Filter sustain", 0.0, 1.0, 0.3, Amount),
        Param::new("f_release", "Filter release", 0.004, 10.0, 0.3, Seconds).log(),
        Param::new("vel_filter", "Velocity to filter", 0.0, 1.0, 0.3, Amount),
        Param::new("attack", "Attack", 0.001, 10.0, 0.003, Seconds).log(),
        Param::new("decay", "Decay", 0.004, 10.0, 0.4, Seconds).log(),
        Param::new("sustain", "Sustain", 0.0, 1.0, 0.8, Amount),
        Param::new("release", "Release", 0.004, 10.0, 0.25, Seconds).log(),
        Param::new("vel_amp", "Velocity to level", 0.0, 1.0, 0.5, Amount),
        Param::new("lfo_rate", "LFO rate", 0.05, 30.0, 5.0, Hz).log(),
        Param::new("lfo_saw", "LFO saw", 0.0, 1.0, 0.0, Switch),
        Param::new("lfo_tri", "LFO triangle", 0.0, 1.0, 1.0, Switch),
        Param::new("lfo_square", "LFO square", 0.0, 1.0, 0.0, Switch),
        Param::new("lfo_initial", "LFO initial amount", 0.0, 1.0, 0.0, Amount),
        Param::new("lfo_freq_a", "LFO to osc A pitch", 0.0, 1.0, 1.0, Switch),
        Param::new("lfo_freq_b", "LFO to osc B pitch", 0.0, 1.0, 1.0, Switch),
        Param::new("lfo_width_a", "LFO to osc A width", 0.0, 1.0, 0.0, Switch),
        Param::new("lfo_width_b", "LFO to osc B width", 0.0, 1.0, 0.0, Switch),
        Param::new("lfo_filter", "LFO to filter", 0.0, 1.0, 0.0, Switch),
        Param::new("voices", "Voices", 1.0, VOICES as f64, 8.0, Count),
        Param::new(
            "unison",
            "Unison voices per note",
            1.0,
            UNISON as f64,
            1.0,
            Count,
        ),
        Param::new("detune", "Unison detune", 0.0, 50.0, 12.0, Cents),
        Param::new("spread", "Unison spread", 0.0, 1.0, 0.5, Amount),
        Param::new("glide", "Glide time", 0.0, 2.0, 0.0, Seconds),
        Param::new("legato", "Legato (one voice)", 0.0, 1.0, 0.0, Switch),
        Param::new("bend", "Bend range", 0.0, 24.0, 2.0, Semitones),
        Param::new("level", "Level", -36.0, 6.0, -10.0, Db),
        Param::new("vintage", "Vintage drift", 0.0, 1.0, 0.4, Amount),
    ]
}

/// The settings in groups, for the controls: (heading, first, last).
pub const GROUPS: &[(&str, usize, usize)] = &[
    ("Oscillator A", at::A_SAW, at::A_SYNC),
    ("Oscillator B", at::B_SAW, at::B_KEYS),
    ("Mixer", at::A_LEVEL, at::NOISE_LEVEL),
    ("Poly-mod", at::PM_ENV, at::PM_FILTER),
    ("Filter", at::BOARD, at::VEL_FILTER),
    ("Amplifier", at::ATTACK, at::VEL_AMP),
    ("LFO", at::LFO_RATE, at::LFO_FILTER),
    ("Playing", at::VOICES, at::VINTAGE),
];

/// Sounds made here, as settings over the defaults: brass, pads, leads
/// and bass, for the content browser and the assistant.
pub fn sounds() -> Vec<crate::Sound> {
    use crate::Sound;
    vec![
        Sound {
            name: "Poly brass",
            settings: &[
                ("b_fine", 0.1),
                ("cutoff", 500.0),
                ("resonance", 0.15),
                ("env_amount", 3.5),
                ("f_attack", 0.05),
                ("f_decay", 0.35),
                ("f_sustain", 0.4),
                ("attack", 0.02),
                ("sustain", 0.9),
                ("release", 0.3),
                ("key_track", 1.0),
            ],
        },
        Sound {
            name: "Warm pad",
            settings: &[
                ("a_saw", 0.0),
                ("a_tri", 1.0),
                ("a_pulse", 1.0),
                ("a_width", 0.3),
                ("b_fine", -0.12),
                ("b_level", 0.7),
                ("board", 1.0),
                ("cutoff", 900.0),
                ("resonance", 0.1),
                ("env_amount", 1.5),
                ("f_attack", 1.2),
                ("f_decay", 2.0),
                ("f_sustain", 0.6),
                ("f_release", 1.5),
                ("attack", 0.8),
                ("decay", 1.0),
                ("sustain", 1.0),
                ("release", 1.8),
                ("lfo_rate", 0.4),
                ("lfo_initial", 0.5),
                ("lfo_freq_a", 0.0),
                ("lfo_freq_b", 0.0),
                ("lfo_width_a", 1.0),
                ("key_track", 2.0),
            ],
        },
        Sound {
            name: "Sync lead",
            settings: &[
                ("a_sync", 1.0),
                ("b_coarse", -12.0),
                ("b_fine", 0.0),
                ("b_level", 0.0),
                ("pm_env", 0.6),
                ("pm_freq", 1.0),
                ("cutoff", 3000.0),
                ("resonance", 0.1),
                ("env_amount", 1.0),
                ("f_decay", 0.6),
                ("f_sustain", 0.1),
                ("sustain", 0.9),
                ("voices", 1.0),
                ("legato", 1.0),
                ("glide", 0.05),
            ],
        },
        Sound {
            name: "Poly bass",
            settings: &[
                ("a_pulse", 1.0),
                ("a_width", 0.4),
                ("b_coarse", -12.0),
                ("b_fine", 0.03),
                ("b_level", 0.8),
                ("cutoff", 350.0),
                ("resonance", 0.25),
                ("env_amount", 2.5),
                ("f_decay", 0.3),
                ("f_sustain", 0.15),
                ("decay", 0.5),
                ("sustain", 0.6),
                ("release", 0.12),
                ("key_track", 1.0),
                ("vel_filter", 0.5),
            ],
        },
        Sound {
            name: "Strings",
            settings: &[
                ("a_saw", 1.0),
                ("a_pulse", 1.0),
                ("a_width", 0.5),
                ("b_saw", 1.0),
                ("b_pulse", 1.0),
                ("b_width", 0.5),
                ("b_fine", 0.15),
                ("board", 1.0),
                ("mode", 1.0),
                ("cutoff", 1400.0),
                ("resonance", 0.3),
                ("env_amount", 0.5),
                ("f_attack", 0.4),
                ("f_sustain", 1.0),
                ("attack", 0.35),
                ("sustain", 1.0),
                ("release", 0.9),
                ("lfo_rate", 0.7),
                ("lfo_initial", 0.6),
                ("lfo_freq_a", 0.0),
                ("lfo_freq_b", 0.0),
                ("lfo_width_a", 1.0),
                ("lfo_width_b", 1.0),
                ("key_track", 2.0),
            ],
        },
        Sound {
            name: "Bell",
            settings: &[
                ("a_tri", 1.0),
                ("a_saw", 0.0),
                ("b_tri", 1.0),
                ("b_saw", 0.0),
                ("b_coarse", 19.0),
                ("b_fine", 0.0),
                ("b_level", 0.0),
                ("pm_osc_b", 0.5),
                ("pm_freq", 1.0),
                ("cutoff", 4000.0),
                ("resonance", 0.0),
                ("env_amount", 2.0),
                ("f_decay", 1.5),
                ("f_sustain", 0.0),
                ("decay", 2.5),
                ("sustain", 0.0),
                ("release", 1.5),
                ("key_track", 2.0),
                ("vel_amp", 0.7),
            ],
        },
        Sound {
            name: "Unison lead",
            settings: &[
                ("b_fine", 0.08),
                ("cutoff", 2200.0),
                ("resonance", 0.2),
                ("env_amount", 2.0),
                ("f_decay", 0.8),
                ("f_sustain", 0.4),
                ("sustain", 0.9),
                ("voices", 1.0),
                ("unison", 6.0),
                ("detune", 14.0),
                ("spread", 0.8),
                ("legato", 1.0),
                ("glide", 0.08),
            ],
        },
        Sound {
            name: "Hollow keys",
            settings: &[
                ("a_saw", 0.0),
                ("a_pulse", 1.0),
                ("a_width", 0.15),
                ("b_saw", 0.0),
                ("b_pulse", 1.0),
                ("b_width", 0.85),
                ("b_coarse", 12.0),
                ("b_fine", 0.04),
                ("b_level", 0.35),
                ("board", 1.0),
                ("cutoff", 1800.0),
                ("resonance", 0.35),
                ("env_amount", 2.0),
                ("f_decay", 0.4),
                ("f_sustain", 0.0),
                ("decay", 1.5),
                ("sustain", 0.2),
                ("release", 0.4),
                ("key_track", 2.0),
                ("vel_filter", 0.6),
                ("vel_amp", 0.7),
            ],
        },
        Sound {
            name: "Pluck",
            settings: &[
                ("b_coarse", 12.0),
                ("b_level", 0.4),
                ("cutoff", 350.0),
                ("resonance", 0.3),
                ("env_amount", 4.5),
                ("f_decay", 0.2),
                ("f_sustain", 0.0),
                ("f_release", 0.2),
                ("attack", 0.002),
                ("decay", 0.5),
                ("sustain", 0.0),
                ("release", 0.35),
                ("vel_filter", 0.5),
            ],
        },
        Sound {
            name: "Sweep pad",
            settings: &[
                ("a_saw", 0.0),
                ("a_pulse", 1.0),
                ("a_width", 0.35),
                ("b_fine", 0.12),
                ("board", 1.0),
                ("cutoff", 400.0),
                ("resonance", 0.6),
                ("env_amount", 2.5),
                ("f_attack", 2.5),
                ("f_decay", 3.0),
                ("f_sustain", 0.4),
                ("f_release", 2.0),
                ("attack", 1.2),
                ("decay", 1.0),
                ("sustain", 1.0),
                ("release", 2.0),
                ("lfo_rate", 0.4),
                ("lfo_initial", 0.15),
                ("lfo_freq_a", 0.0),
                ("lfo_freq_b", 0.0),
                ("lfo_width_a", 1.0),
                ("level", -16.0),
            ],
        },
        Sound {
            name: "Organ",
            settings: &[
                ("a_saw", 0.0),
                ("a_pulse", 1.0),
                ("a_width", 0.5),
                ("b_saw", 0.0),
                ("b_pulse", 1.0),
                ("b_coarse", 12.0),
                ("b_fine", 0.0),
                ("b_level", 0.5),
                ("cutoff", 2500.0),
                ("resonance", 0.0),
                ("env_amount", 0.0),
                ("attack", 0.003),
                ("decay", 0.1),
                ("sustain", 1.0),
                ("release", 0.06),
                ("lfo_rate", 6.0),
                ("lfo_initial", 0.03),
                ("level", -16.0),
            ],
        },
    ]
}

/// Settings are read, and the filters set, this often.
const CHUNK: usize = 16;

/// What the LFO reaches at full depth: a fifth of pitch either way,
/// nearly the whole pulse width, three octaves of cutoff.
const LFO_PITCH: f32 = 7.0 / 12.0;
const LFO_WIDTH: f32 = 0.45;
const LFO_FILTER: f32 = 3.0;
/// What poly-mod reaches at full amount: four octaves of oscillator A
/// from the envelope, two from oscillator B, half the pulse width,
/// four octaves of cutoff.
const PM_ENV_PITCH: f32 = 4.0;
const PM_OSC_PITCH: f32 = 2.0;
const PM_WIDTH: f32 = 0.5;
const PM_FILTER: f32 = 4.0;

/// Which of the filter boards a voice plays through.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Board {
    Cascade,
    Svf(SvfOut),
}

fn board(choice: f32, mode: f32) -> Board {
    if choice < 0.5 {
        Board::Cascade
    } else {
        Board::Svf(match mode as u32 {
            1 => SvfOut::Morph(0.5),
            2 => SvfOut::Morph(1.0),
            3 => SvfOut::Band,
            _ => SvfOut::Morph(0.0),
        })
    }
}

/// One oscillator: its three shapes on the one phase.
#[derive(Clone, Copy, Debug)]
struct Pair {
    saw: Osc,
    tri: Osc,
    pulse: Osc,
}

impl Pair {
    fn new(phase: f32) -> Pair {
        let mut p = Pair {
            saw: Osc::new(Wave::Saw),
            tri: Osc::new(Wave::Triangle),
            pulse: Osc::new(Wave::Pulse),
        };
        p.saw.reset(phase);
        p.tri.reset(phase);
        p.pulse.reset(phase);
        p
    }

    #[inline]
    fn set(&mut self, dt: f32, width: f32) {
        self.saw.dt = dt;
        self.tri.dt = dt;
        self.pulse.dt = dt;
        self.pulse.width = width;
    }

    /// Runs the three shapes a sample and sums the ones switched on;
    /// `sync` resets them at a master's wrap.
    #[inline]
    fn tick(&mut self, on: [bool; 3], sync: Option<f32>) -> f32 {
        let s = self.saw.tick(sync);
        let t = self.tri.tick(sync);
        let p = self.pulse.tick(sync);
        let mut v = 0.0;
        if on[0] {
            v += s;
        }
        if on[1] {
            v += t;
        }
        if on[2] {
            v += p;
        }
        v
    }

    #[inline]
    fn wrapped(&self) -> Option<f32> {
        self.saw.wrapped
    }
}

/// One of the voices.
#[derive(Clone, Copy, Debug)]
struct Voice {
    on: bool,
    note: u8,
    /// The pitch now, gliding towards `note`.
    pitch: f32,
    velocity: f32,
    /// Its place in a unison stack: (index, stack size).
    stack: (usize, usize),
    a: Pair,
    b: Pair,
    drift: [Drift; 2],
    /// Scatter from voice to voice, −1 to 1, by the vintage amount:
    /// the cutoff and the envelope speeds.
    scatter: f32,
    fenv: Analog,
    aenv: Analog,
    cascade: Cascade,
    svf: Svf,
    /// Oscillator B's last sample, the poly-mod source.
    osc_b: f32,
}

/// The synth on the audio thread.
pub struct Oracle {
    params: Live,
    rate: f32,
    voices: Vec<Voice>,
    alloc: Allocator,
    actions: Vec<Action>,
    bend: f32,
    /// The mod wheel, 0 to 1.
    wheel: f32,
    noise: Noise,
    lfo_phase: f32,
    /// The output's DC blocker on each side: the last input and output.
    dc: [(f32, f32); 2],
}

impl Oracle {
    pub fn new(rate: u32, params: impl Into<Live>) -> Oracle {
        let rate = rate.max(1) as f32;
        let mut noise = Noise::new(0x0ac1_e5ed);
        let voices = (0..VOICES)
            .map(|v| Voice {
                on: false,
                note: 60,
                pitch: 60.0,
                velocity: 0.0,
                stack: (0, 1),
                // Each voice's oscillators sit at their own phases, as
                // free-running ones would.
                a: Pair::new(noise.tick() * 0.5 + 0.5),
                b: Pair::new(noise.tick() * 0.5 + 0.5),
                drift: [
                    Drift::new(v as u32 * 2 + 101),
                    Drift::new(v as u32 * 2 + 102),
                ],
                scatter: noise.tick(),
                fenv: Analog::IDLE,
                aenv: Analog::IDLE,
                cascade: Cascade::new(),
                svf: Svf::new(),
                osc_b: 0.0,
            })
            .collect();
        Oracle {
            params: params.into(),
            rate,
            voices,
            alloc: Allocator::new(VOICES),
            actions: Vec::with_capacity(2 * VOICES),
            bend: 0.0,
            wheel: 0.0,
            noise,
            lfo_phase: 0.0,
            dc: [(0.0, 0.0); 2],
        }
    }

    fn settings(&self) -> VoiceSettings {
        let notes = (self.params.read(at::VOICES) as usize).clamp(1, VOICES);
        VoiceSettings {
            notes,
            unison: (self.params.read(at::UNISON) as usize).clamp(1, UNISON),
            priority: Priority::Last,
            legato: notes == 1 && self.params.read(at::LEGATO) >= 0.5,
        }
    }

    fn apply(&mut self) {
        let glide = self.params.read(at::GLIDE) > 0.0;
        let unison = (self.params.read(at::UNISON) as usize).clamp(1, UNISON);
        // Which voice of its stack each start is: the count of starts
        // on the same note before it in this batch.
        let mut started: [(u8, usize); VOICES] = [(0, 0); VOICES];
        let mut starts = 0;
        for a in self.actions.drain(..) {
            match a {
                Action::Start {
                    voice,
                    note,
                    velocity,
                    fresh,
                } => {
                    let k = started[..starts].iter().filter(|s| s.0 == note).count();
                    if starts < VOICES {
                        started[starts] = (note, k);
                        starts += 1;
                    }
                    let v = &mut self.voices[voice];
                    v.note = note;
                    v.velocity = velocity.clamp(1, 127) as f32 / 127.0;
                    v.stack = (k.min(unison - 1), unison);
                    if fresh {
                        // The oscillators keep running where they
                        // stopped; the filters start clean.
                        v.cascade.reset();
                        v.svf.reset();
                        v.aenv = Analog::IDLE;
                        v.fenv = Analog::IDLE;
                    }
                    // Glide in every voice, from the note it last
                    // played, as the classic's did.
                    if !glide {
                        v.pitch = note as f32;
                    }
                    v.aenv.trigger();
                    v.fenv.trigger();
                    v.on = true;
                }
                Action::Retune { voice, note } => self.voices[voice].note = note,
                Action::Release { voice } => {
                    self.voices[voice].aenv.release();
                    self.voices[voice].fenv.release();
                }
            }
        }
    }

    fn event(&mut self, e: &Event) {
        let s = self.settings();
        match e.kind {
            Kind::NoteOn {
                pitch, velocity, ..
            } => self.alloc.note_on(pitch, velocity, &s, &mut self.actions),
            Kind::NoteOff { pitch, .. } => self.alloc.note_off(pitch, &s, &mut self.actions),
            Kind::PitchBend(b) => self.bend = b as f32 / 8192.0,
            Kind::Control { number: 1, value } => self.wheel = value.min(127) as f32 / 127.0,
            Kind::Control { number: 64, value } => self.alloc.pedal(value >= 64, &mut self.actions),
            Kind::Control { number: 120, .. } | Kind::AllNotesOff => {
                self.alloc.all_off(&mut self.actions);
            }
            _ => {}
        }
        self.apply();
    }

    fn run(&mut self, from: usize, to: usize, left: &mut [f32], right: &mut [f32]) {
        let mut at = from;
        while at < to {
            let end = (at + CHUNK).min(to);
            self.chunk(at, end, left, right);
            at = end;
        }
    }

    /// The LFO over a chunk: its three shapes summed as switched on,
    /// −1 to 1, at the chunk's start, and the phase advanced.
    fn lfo(&mut self, n: f32) -> f32 {
        let p = &self.params;
        let ph = self.lfo_phase;
        let mut v = 0.0;
        let mut count = 0.0;
        if p.read(at::LFO_SAW) >= 0.5 {
            v += 2.0 * ph - 1.0;
            count += 1.0;
        }
        if p.read(at::LFO_TRI) >= 0.5 {
            v += 1.0 - 4.0 * (ph - 0.5).abs();
            count += 1.0;
        }
        if p.read(at::LFO_SQUARE) >= 0.5 {
            v += if ph < 0.5 { 1.0 } else { -1.0 };
            count += 1.0;
        }
        self.lfo_phase = (ph + p.read(at::LFO_RATE) * n / self.rate).fract();
        if count > 0.0 { v / count } else { 0.0 }
    }

    fn chunk(&mut self, from: usize, to: usize, left: &mut [f32], right: &mut [f32]) {
        let n = (to - from) as f32;
        let lfo = self.lfo(n);
        if !self.voices.iter().any(|v| v.on) {
            return;
        }
        let p = &self.params;
        let r = self.rate;
        let glide = Glide {
            mode: GlideMode::Time,
            seconds: p.read(at::GLIDE),
        };
        let vintage = p.read(at::VINTAGE);
        let bend = self.bend * p.read(at::BEND);
        let a_on = [
            p.read(at::A_SAW) >= 0.5,
            p.read(at::A_TRI) >= 0.5,
            p.read(at::A_PULSE) >= 0.5,
        ];
        let b_on = [
            p.read(at::B_SAW) >= 0.5,
            p.read(at::B_TRI) >= 0.5,
            p.read(at::B_PULSE) >= 0.5,
        ];
        let sync = p.read(at::A_SYNC) >= 0.5;
        let a_width = p.read(at::A_WIDTH);
        let b_width = p.read(at::B_WIDTH);
        let b_semis = p.read(at::B_COARSE) + p.read(at::B_FINE);
        let b_low = p.read(at::B_LOW) >= 0.5;
        let b_keys = p.read(at::B_KEYS) >= 0.5;
        let levels = [
            p.read(at::A_LEVEL),
            p.read(at::B_LEVEL),
            p.read(at::NOISE_LEVEL),
        ];
        // The LFO's reach, from the initial amount and the wheel.
        let depth = (self.wheel + p.read(at::LFO_INITIAL)).clamp(0.0, 1.0) * lfo;
        let lfo_a = if p.read(at::LFO_FREQ_A) >= 0.5 {
            depth * LFO_PITCH
        } else {
            0.0
        };
        let lfo_b = if p.read(at::LFO_FREQ_B) >= 0.5 {
            depth * LFO_PITCH
        } else {
            0.0
        };
        let lfo_wa = if p.read(at::LFO_WIDTH_A) >= 0.5 {
            depth * LFO_WIDTH
        } else {
            0.0
        };
        let lfo_wb = if p.read(at::LFO_WIDTH_B) >= 0.5 {
            depth * LFO_WIDTH
        } else {
            0.0
        };
        let lfo_f = if p.read(at::LFO_FILTER) >= 0.5 {
            depth * LFO_FILTER
        } else {
            0.0
        };
        // Poly-mod: the filter envelope and oscillator B, each by its
        // amount, to oscillator A's pitch and width and the cutoff.
        let pm_env = p.read(at::PM_ENV);
        let pm_b = p.read(at::PM_OSC_B);
        let pm_freq = p.read(at::PM_FREQ) >= 0.5;
        let pm_width = p.read(at::PM_WIDTH) >= 0.5;
        let pm_filter = p.read(at::PM_FILTER) >= 0.5;
        // Oscillator B into the filter is audio-rate: the filter is
        // then set every sample rather than every chunk.
        let per_sample = pm_filter && pm_b > 0.0;
        let which = board(p.read(at::BOARD), p.read(at::MODE));
        let cutoff = p.read(at::CUTOFF);
        let resonance = p.read(at::RESONANCE);
        let env_amount = p.read(at::ENV_AMOUNT);
        let track = [0.0, 0.5, 1.0][(p.read(at::KEY_TRACK) as usize).min(2)];
        let vel_f = p.read(at::VEL_FILTER);
        let vel_a = p.read(at::VEL_AMP);
        let fshape = Shape::adsr(
            p.read(at::F_ATTACK),
            p.read(at::F_DECAY),
            p.read(at::F_SUSTAIN),
            p.read(at::F_RELEASE),
        );
        let ashape = Shape::adsr(
            p.read(at::ATTACK),
            p.read(at::DECAY),
            p.read(at::SUSTAIN),
            p.read(at::RELEASE),
        );
        let detune = p.read(at::DETUNE);
        let spread = p.read(at::SPREAD);
        let gain = db(p.read(at::LEVEL));
        let dc_k = 1.0 - std::f32::consts::TAU * 5.0 / r;
        let noise = &mut self.noise;
        let alloc = &mut self.alloc;

        for (vi, v) in self.voices.iter_mut().enumerate() {
            if !v.on {
                continue;
            }
            // The envelopes, a little faster or slower in each voice
            // by the vintage amount.
            let stretch = 1.0 + 0.08 * vintage * v.scatter;
            let scale = |s: Shape| Shape {
                attack: s.attack * stretch,
                decay: s.decay * stretch,
                release: s.release * stretch,
                ..s
            };
            let fsteps = Steps::new(scale(fshape), r);
            let asteps = Steps::new(scale(ashape), r);
            v.pitch = glide.step(v.pitch, v.note as f32, n, r);
            // Unison: the stack spread evenly either side of the note.
            let (k, size) = v.stack;
            let place = if size > 1 {
                (k as f32 / (size - 1) as f32) * 2.0 - 1.0
            } else {
                0.0
            };
            let pitch = v.pitch + bend + place * detune / 100.0;
            let pans = pan(place * spread);
            let drift_a = v.drift[0].next(vintage, n, r) / 100.0;
            let drift_b = v.drift[1].next(vintage, n, r) / 100.0;
            let a_hz = hz(pitch + drift_a + lfo_a);
            let b_hz = if b_low {
                4.0 * ((b_semis + lfo_b * 12.0) / 12.0).exp2()
            } else if b_keys {
                hz(pitch + b_semis + drift_b + lfo_b)
            } else {
                hz(60.0 + bend + b_semis + drift_b + lfo_b)
            };
            let dt_a = (a_hz / r).min(0.5);
            let dt_b = (b_hz / r).min(0.5);
            let vel_fenv = env_amount * (1.0 - vel_f + vel_f * v.velocity);
            let amp_vel = 1.0 - vel_a + vel_a * v.velocity;
            // The filter: the knob, the key, the envelope, the LFO and
            // the voice's own scatter, once a chunk unless oscillator
            // B modulates it.
            let base = track * (pitch - 60.0) / 12.0 + lfo_f + 0.04 * vintage * v.scatter;
            let set = |v: &mut Voice, octaves: f32| {
                let fc = cutoff * octaves.exp2();
                match which {
                    Board::Cascade => v.cascade.set(Poles::Four, fc, resonance, 1.0, 0.5, r),
                    Board::Svf(out) => v.svf.set(out, fc, resonance, 1.0, r),
                }
            };
            if !per_sample {
                let env = v.fenv.level;
                let pm = if pm_filter {
                    pm_env * env * PM_FILTER
                } else {
                    0.0
                };
                set(v, base + vel_fenv * env + pm);
            }
            let mut rest = 0;
            for i in from..to {
                let a = v.aenv.next(&asteps) * amp_vel;
                let env = v.fenv.next(&fsteps);
                if !v.aenv.active() {
                    v.on = false;
                    alloc.free(vi);
                    rest = i;
                    break;
                }
                // Poly-mod, from the envelope now and oscillator B's
                // last sample.
                let m_env = pm_env * env;
                let m_b = pm_b * v.osc_b;
                let (mut fa, mut wa) = (dt_a, a_width + lfo_wa);
                if pm_freq {
                    fa = (dt_a * (m_env * PM_ENV_PITCH + m_b * PM_OSC_PITCH).exp2()).min(0.5);
                }
                if pm_width {
                    wa += (m_env + m_b) * PM_WIDTH;
                }
                v.a.set(fa, wa.clamp(0.03, 0.97));
                v.b.set(dt_b, (b_width + lfo_wb).clamp(0.03, 0.97));
                if per_sample {
                    set(v, base + vel_fenv * env + (m_env + m_b) * PM_FILTER);
                }
                let ob = v.b.tick(b_on, None);
                v.osc_b = ob;
                let oa = v.a.tick(a_on, if sync { v.b.wrapped() } else { None });
                let mixed = levels[0] * oa + levels[1] * ob + levels[2] * noise.tick();
                let filtered = match which {
                    Board::Cascade => v.cascade.run(mixed),
                    Board::Svf(_) => v.svf.run(mixed),
                };
                // The amplifier's own soft limit, then the level.
                let y = soft(filtered * a) * gain;
                left[i] += y * pans.0;
                right[i] += y * pans.1;
            }
            let _ = rest;
        }
        // The DC blockers, one-pole high-passes at 5 Hz.
        for i in from..to {
            for (side, dc) in [&mut left[i], &mut right[i]].into_iter().zip(&mut self.dc) {
                let y = *side;
                let out = y - dc.0 + dc_k * dc.1;
                *dc = (y, out);
                *side = out;
            }
        }
    }
}

impl Instrument for Oracle {
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
    use crate::vintage::measure;
    use std::sync::Arc;

    const RATE: f32 = 48_000.0;

    fn synth() -> Oracle {
        let s = Oracle::new(RATE as u32, Arc::new(Params::new(params())));
        s.params.set(at::VINTAGE, 0.0);
        s.params.set(at::B_FINE, 0.0);
        s
    }

    fn render(s: &mut Oracle, events: &[Event], n: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        s.process(events, &mut l, &mut r);
        (l, r)
    }

    fn mono(s: &mut Oracle, events: &[Event], n: usize) -> Vec<f32> {
        let (l, r) = render(s, events, n);
        assert!(
            l.iter().zip(&r).all(|(a, b)| (a - b).abs() < 1e-6),
            "not the same both sides"
        );
        l
    }

    /// The level of `hz` in the second half of `x`.
    fn level_at(x: &[f32], hz: f32) -> f32 {
        let (mut re, mut im) = (0.0f32, 0.0f32);
        let half = x.len() / 2;
        for (i, &v) in x.iter().enumerate().skip(half) {
            let p = std::f32::consts::TAU * hz * i as f32 / RATE;
            re += v * p.cos();
            im += v * p.sin();
        }
        (re * re + im * im).sqrt() * 2.0 / (x.len() - half) as f32
    }

    fn sounding(s: &Oracle) -> usize {
        s.voices.iter().filter(|v| v.on).count()
    }

    #[test]
    fn a_note_sounds_then_dies_away() {
        let mut s = synth();
        let x = mono(&mut s, &[Event::note_on(100, 0, 48, 100, 0)], 4800);
        assert_eq!(measure::peak(&x[..100]), 0.0);
        assert!(measure::peak(&x[100..]) > 0.05);
        let x = mono(&mut s, &[Event::note_off(0, 0, 48, 0)], 96_000);
        assert_eq!(measure::peak(&x[90_000..]), 0.0);
        assert_eq!(sounding(&s), 0);
    }

    #[test]
    fn chords_take_a_voice_each_up_to_the_voice_count() {
        let mut s = synth();
        s.params.set(at::VOICES, 4.0);
        let notes: Vec<Event> = (0..6)
            .map(|i| Event::note_on(i * 10, 0, 48 + i as u8 * 3, 100, i))
            .collect();
        let x = mono(&mut s, &notes, 4800);
        assert!(measure::peak(&x) > 0.1);
        // Six notes on four voices: the two oldest went into release,
        // but may still sound.
        let held = s.alloc.voices();
        assert_eq!(held, VOICES);
        assert!(sounding(&s) >= 4);
        let mut held = 0;
        for v in 0..VOICES {
            if s.alloc.note_of(v).is_some() && s.voices[v].aenv.held() {
                held += 1;
            }
        }
        assert_eq!(held, 4);
    }

    #[test]
    fn its_pitch_follows_the_note_and_osc_b_its_frequency() {
        let mut s = synth();
        s.params.set(at::B_LEVEL, 0.0);
        s.params.set(at::CUTOFF, 18_000.0);
        s.params.set(at::ENV_AMOUNT, 0.0);
        let x = mono(&mut s, &[Event::note_on(0, 0, 69, 127, 0)], 48_000);
        let f = measure::ring_hz(&x, RATE);
        assert!((f - 440.0).abs() < 8.0, "{f}");
        let mut s = synth();
        s.params.set(at::A_LEVEL, 0.0);
        s.params.set(at::B_COARSE, -12.0);
        s.params.set(at::CUTOFF, 18_000.0);
        s.params.set(at::ENV_AMOUNT, 0.0);
        let x = mono(&mut s, &[Event::note_on(0, 0, 69, 127, 0)], 48_000);
        let f = measure::ring_hz(&x, RATE);
        assert!((f - 220.0).abs() < 5.0, "{f}");
    }

    #[test]
    fn hard_sync_puts_osc_b_s_pitch_under_osc_a_s_tone() {
        let mut s = synth();
        s.params.set(at::A_SYNC, 1.0);
        s.params.set(at::B_LEVEL, 0.0);
        s.params.set(at::B_COARSE, -7.0);
        s.params.set(at::CUTOFF, 18_000.0);
        s.params.set(at::ENV_AMOUNT, 0.0);
        let x = mono(&mut s, &[Event::note_on(0, 0, 69, 127, 0)], 48_000);
        // The period is B's (a fifth under, 293.7 Hz), with A's tone
        // (440 Hz) on top of it.
        let f0 = hz(62.0);
        let fund = level_at(&x, f0);
        assert!(fund > 0.02, "{fund}");
        assert!(level_at(&x, f0 * 2.0) > 0.01 && level_at(&x, f0 * 3.0) > 0.01);
        let mut s = synth();
        s.params.set(at::B_LEVEL, 0.0);
        s.params.set(at::B_COARSE, -7.0);
        s.params.set(at::CUTOFF, 18_000.0);
        s.params.set(at::ENV_AMOUNT, 0.0);
        let plain = mono(&mut s, &[Event::note_on(0, 0, 69, 127, 0)], 48_000);
        assert!(level_at(&plain, f0) < 0.1 * fund);
    }

    #[test]
    fn the_two_boards_roll_off_differently_and_sing() {
        let mut s = synth();
        s.params.set(at::CUTOFF, 400.0);
        s.params.set(at::ENV_AMOUNT, 0.0);
        s.params.set(at::KEY_TRACK, 0.0);
        s.params.set(at::B_LEVEL, 0.0);
        let four = mono(&mut s, &[Event::note_on(0, 0, 45, 100, 0)], 48_000);
        s.params.set(at::BOARD, 1.0);
        let mut s2 = synth();
        s2.params.set(at::CUTOFF, 400.0);
        s2.params.set(at::ENV_AMOUNT, 0.0);
        s2.params.set(at::KEY_TRACK, 0.0);
        s2.params.set(at::B_LEVEL, 0.0);
        s2.params.set(at::BOARD, 1.0);
        let two = mono(&mut s2, &[Event::note_on(0, 0, 45, 100, 0)], 48_000);
        let f0 = hz(45.0);
        // The 16th harmonic (1760 Hz, two octaves over the cutoff) is
        // much further down on the 4-pole board than the 2-pole one.
        let ratio = |x: &[f32]| level_at(x, f0 * 16.0) / level_at(x, f0);
        assert!(
            ratio(&four) < 0.2 * ratio(&two),
            "{} {}",
            ratio(&four),
            ratio(&two)
        );
        // The state-variable board's high output keeps the top instead.
        s2.params.set(at::MODE, 2.0);
        let high = mono(&mut s2, &[], 48_000);
        assert!(ratio(&high) > 5.0 * ratio(&two), "{}", ratio(&high));
        // Both sing at the cutoff with the resonance full and nothing in.
        for b in [0.0, 1.0] {
            let mut s = synth();
            s.params.set(at::BOARD, b);
            s.params.set(at::MODE, 0.0);
            s.params.set(at::CUTOFF, 1000.0);
            s.params.set(at::RESONANCE, 1.0);
            s.params.set(at::ENV_AMOUNT, 0.0);
            s.params.set(at::KEY_TRACK, 0.0);
            s.params.set(at::A_LEVEL, 0.02);
            s.params.set(at::B_LEVEL, 0.0);
            s.params.set(at::SUSTAIN, 1.0);
            let x = mono(&mut s, &[Event::note_on(0, 0, 30, 100, 0)], 96_000);
            let f = measure::ring_hz(&x[48_000..], RATE);
            assert!((f / 1000.0 - 1.0).abs() < 0.05, "board {b} sings at {f}");
        }
    }

    #[test]
    fn the_filter_envelope_opens_the_filter_and_velocity_scales_it() {
        let bright = |x: &[f32]| {
            let f0 = hz(45.0);
            (4..12).map(|h| level_at(x, f0 * h as f32)).sum::<f32>() / level_at(x, f0).max(1e-6)
        };
        let mut s = synth();
        s.params.set(at::CUTOFF, 300.0);
        s.params.set(at::ENV_AMOUNT, 4.0);
        s.params.set(at::F_SUSTAIN, 1.0);
        s.params.set(at::VEL_FILTER, 1.0);
        s.params.set(at::B_LEVEL, 0.0);
        let loud = mono(&mut s, &[Event::note_on(0, 0, 45, 127, 0)], 48_000);
        let mut s = synth();
        s.params.set(at::CUTOFF, 300.0);
        s.params.set(at::ENV_AMOUNT, 4.0);
        s.params.set(at::F_SUSTAIN, 1.0);
        s.params.set(at::VEL_FILTER, 1.0);
        s.params.set(at::B_LEVEL, 0.0);
        let quiet = mono(&mut s, &[Event::note_on(0, 0, 45, 20, 0)], 48_000);
        let mut s = synth();
        s.params.set(at::CUTOFF, 300.0);
        s.params.set(at::ENV_AMOUNT, 0.0);
        s.params.set(at::B_LEVEL, 0.0);
        let closed = mono(&mut s, &[Event::note_on(0, 0, 45, 127, 0)], 48_000);
        assert!(
            bright(&loud) > 3.0 * bright(&quiet),
            "{} {}",
            bright(&loud),
            bright(&quiet)
        );
        assert!(
            bright(&quiet) > 1.5 * bright(&closed),
            "{} {}",
            bright(&quiet),
            bright(&closed)
        );
    }

    #[test]
    fn poly_mod_from_osc_b_makes_sidebands_on_osc_a() {
        let mut s = synth();
        s.params.set(at::B_LEVEL, 0.0);
        s.params.set(at::B_COARSE, 7.0);
        s.params.set(at::CUTOFF, 18_000.0);
        s.params.set(at::ENV_AMOUNT, 0.0);
        s.params.set(at::A_SAW, 0.0);
        s.params.set(at::A_TRI, 1.0);
        s.params.set(at::B_SAW, 0.0);
        s.params.set(at::B_TRI, 1.0);
        let plain = mono(&mut s, &[Event::note_on(0, 0, 69, 127, 0)], 48_000);
        s.params.set(at::PM_OSC_B, 0.4);
        s.params.set(at::PM_FREQ, 1.0);
        let modded = mono(&mut s, &[], 48_000);
        // A at 440 swung nearly an octave either way by B at 659, through
        // the exponential pitch input as the classic's: the tone is no
        // longer at 440 and is clangorous, while the level holds.
        assert!(level_at(&modded, 440.0) < 0.3 * level_at(&plain, 440.0));
        assert!(measure::peak(&modded) > 0.5 * measure::peak(&plain));
        // And to the pulse width: the duty changes with the envelope.
        let mut s = synth();
        s.params.set(at::B_LEVEL, 0.0);
        s.params.set(at::A_SAW, 0.0);
        s.params.set(at::A_PULSE, 1.0);
        s.params.set(at::CUTOFF, 18_000.0);
        s.params.set(at::ENV_AMOUNT, 0.0);
        s.params.set(at::F_SUSTAIN, 1.0);
        let square = mono(&mut s, &[Event::note_on(0, 0, 69, 127, 0)], 48_000);
        s.params.set(at::PM_ENV, 0.8);
        s.params.set(at::PM_WIDTH, 1.0);
        let narrow = mono(&mut s, &[], 48_000);
        // A square has no second harmonic; a narrow pulse does.
        assert!(level_at(&narrow, 880.0) > 4.0 * level_at(&square, 880.0).max(1e-4));
    }

    #[test]
    fn the_lfo_wobbles_the_pitch_by_the_wheel() {
        let mut s = synth();
        s.params.set(at::B_LEVEL, 0.0);
        s.params.set(at::CUTOFF, 18_000.0);
        s.params.set(at::ENV_AMOUNT, 0.0);
        s.params.set(at::LFO_RATE, 6.0);
        let still = mono(&mut s, &[Event::note_on(0, 0, 69, 127, 0)], 48_000);
        let wobbly = mono(
            &mut s,
            &[Event {
                frame: 0,
                channel: 0,
                kind: Kind::Control {
                    number: 1,
                    value: 127,
                },
            }],
            48_000,
        );
        assert!(level_at(&wobbly, 440.0) < 0.5 * level_at(&still, 440.0));
        assert!(measure::peak(&wobbly) > 0.05);
        // Nothing when the LFO goes nowhere.
        s.params.set(at::LFO_FREQ_A, 0.0);
        s.params.set(at::LFO_FREQ_B, 0.0);
        let off = mono(&mut s, &[], 48_000);
        assert!(level_at(&off, 440.0) > 0.8 * level_at(&still, 440.0));
    }

    #[test]
    fn unison_stacks_voices_on_one_note_and_spreads_them() {
        let mut s = synth();
        s.params.set(at::UNISON, 4.0);
        s.params.set(at::SPREAD, 1.0);
        s.params.set(at::DETUNE, 20.0);
        let (l, r) = render(&mut s, &[Event::note_on(0, 0, 57, 100, 0)], 9600);
        assert_eq!(sounding(&s), 4);
        let stacks: Vec<usize> = (0..4).map(|v| s.voices[v].stack.0).collect();
        assert_eq!(stacks, vec![0, 1, 2, 3]);
        assert!(l.iter().zip(&r).any(|(a, b)| (a - b).abs() > 1e-3));
        assert!(measure::peak(&l) > 0.05 && measure::peak(&r) > 0.05);
        // One voice per note: the same on both sides.
        let mut s = synth();
        s.params.set(at::SPREAD, 1.0);
        mono(&mut s, &[Event::note_on(0, 0, 57, 100, 0)], 9600);
    }

    #[test]
    fn one_voice_with_legato_glides_between_held_notes() {
        let mut s = synth();
        s.params.set(at::VOICES, 1.0);
        s.params.set(at::LEGATO, 1.0);
        s.params.set(at::GLIDE, 0.2);
        mono(&mut s, &[Event::note_on(0, 0, 48, 100, 0)], 4800);
        let before = s.voices[0].aenv.level;
        mono(&mut s, &[Event::note_on(0, 0, 60, 100, 1)], 480);
        // No restart: the envelope carries on, and the pitch is on its
        // way.
        assert!(s.voices[0].aenv.level >= before * 0.9);
        assert_eq!(sounding(&s), 1);
        assert_eq!(s.voices[0].note, 60);
        assert!(
            s.voices[0].pitch > 48.5 && s.voices[0].pitch < 59.0,
            "{}",
            s.voices[0].pitch
        );
        mono(&mut s, &[], 48_000);
        assert!((s.voices[0].pitch - 60.0).abs() < 0.05);
    }

    #[test]
    fn vintage_scatters_voices_so_they_never_line_up() {
        let mut s = synth();
        s.params.set(at::VINTAGE, 1.0);
        s.params.set(at::VOICES, 2.0);
        s.params.set(at::B_LEVEL, 0.0);
        s.params.set(at::CUTOFF, 18_000.0);
        s.params.set(at::ENV_AMOUNT, 0.0);
        let (l, _) = render(&mut s, &[Event::note_on(0, 0, 69, 100, 0)], 48_000);
        let one = measure::ring_hz(&l, RATE);
        let mut s = synth();
        s.params.set(at::VINTAGE, 1.0);
        s.params.set(at::B_LEVEL, 0.0);
        s.params.set(at::CUTOFF, 18_000.0);
        s.params.set(at::ENV_AMOUNT, 0.0);
        // The second voice sounds alone: it has its own offset.
        render(&mut s, &[Event::note_on(0, 0, 40, 100, 0)], 48);
        s.event(&Event::note_off(0, 0, 40, 0));
        mono(&mut s, &[], 96_000);
        let (l, _) = render(&mut s, &[Event::note_on(0, 0, 69, 100, 1)], 48_000);
        let two = measure::ring_hz(&l, RATE);
        assert!(
            (one - 440.0).abs() < 15.0 && (two - 440.0).abs() < 15.0,
            "{one} {two}"
        );
        assert!(s.voices[0].scatter != s.voices[1].scatter);
    }

    #[test]
    fn every_sound_plays() {
        for sound in sounds() {
            let mut s = synth();
            for (k, v) in sound.settings {
                assert!(s.params.shared().index(k).is_some(), "{}: {k}", sound.name);
                s.params.shared().set_key(k, *v);
            }
            let (l, r) = render(&mut s, &[Event::note_on(0, 0, 55, 100, 0)], 48_000);
            assert!(
                measure::peak(&l) > 0.02 && measure::peak(&r) > 0.02,
                "{}",
                sound.name
            );
            assert!(
                l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 4.0),
                "{}",
                sound.name
            );
        }
    }

    /// A fixed patch renders to a known spectrum, so a later change to
    /// the toolkit can't silently alter the sound. The levels are those
    /// measured when the synth was built; a change that is meant to
    /// alter it updates them after listening.
    #[test]
    fn a_fixed_patch_renders_to_its_known_spectrum() {
        let mut s = synth();
        for (k, v) in sounds()[0].settings {
            s.params.shared().set_key(k, *v);
        }
        s.params.set(at::VINTAGE, 0.0);
        s.params.set(at::B_FINE, 0.0);
        s.params.set(at::F_SUSTAIN, 1.0);
        s.params.set(at::SUSTAIN, 1.0);
        let x = mono(&mut s, &[Event::note_on(0, 0, 48, 100, 0)], 48_000);
        let f0 = hz(48.0);
        let got: Vec<f32> = (1..=8)
            .map(|h| 20.0 * level_at(&x, f0 * h as f32).max(1e-9).log10())
            .collect();
        for (h, (g, k)) in got.iter().zip(GOLDEN).enumerate() {
            assert!(
                (g - k).abs() < 1.5,
                "harmonic {}: {g:.1} dB, was {k:.1}: {got:?}",
                h + 1
            );
        }
    }

    /// The poly brass at C3's harmonics 1 to 8, in dB, held after the
    /// envelopes have settled.
    const GOLDEN: [f32; 8] = [-18.0, -34.7, -39.8, -34.0, -32.6, -33.4, -36.3, -42.8];

    /// The plan's CPU budget: the default eight voices at 48 kHz, a
    /// small fraction of one core.
    #[test]
    #[cfg_attr(debug_assertions, ignore = "timed in release builds only")]
    fn eight_voices_cost_little() {
        let mut s = synth();
        s.params.set(at::A_PULSE, 1.0);
        s.params.set(at::B_PULSE, 1.0);
        s.params.set(at::NOISE_LEVEL, 0.1);
        s.params.set(at::LFO_INITIAL, 0.3);
        s.params.set(at::LFO_WIDTH_A, 1.0);
        s.params.set(at::PM_ENV, 0.3);
        s.params.set(at::PM_FREQ, 1.0);
        let seconds = 2.0;
        let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
        let notes: Vec<Event> = (0..8)
            .map(|i| Event::note_on(0, 0, 48 + i as u8 * 2, 100, i))
            .collect();
        let start = std::time::Instant::now();
        s.process(&notes, &mut l, &mut r);
        for _ in 0..(RATE * seconds / 512.0) as usize {
            s.process(&[], &mut l, &mut r);
        }
        let used = start.elapsed().as_secs_f64() / seconds as f64;
        println!("eight voices: {:.2}% of one core", used * 100.0);
        assert_eq!(sounding(&s), 8);
        assert!(used < 0.25, "{:.2}% of one core", used * 100.0);
    }
}
