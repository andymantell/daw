// SPDX-License-Identifier: GPL-3.0-or-later
//! Lucky Seven: the six-operator FM synth, built the way the 1983
//! Japanese classic was (`docs/plans/classic-synths.md`, S8), for
//! electric pianos, bells, FM bass, brass and plucks. Six sine
//! operators, each with a frequency as a ratio of the note or fixed in
//! Hz, a fine ratio and a detune, an output level on the 0 to 99 scale,
//! an eight-value rate/level envelope, level scaling either side of a
//! break point with linear or exponential curves, rate scaling up the
//! keyboard, a velocity sensitivity and an amplitude-modulation
//! sensitivity; thirty-two algorithms saying which operator modulates
//! which and which are heard, with feedback round one of them; an LFO
//! with six shapes to pitch and to level, delayed and keyed; a pitch
//! envelope with four rates and four levels; a transpose. Sixteen
//! voices, or one with glide.
//!
//! The scales are the ones the format uses (levels, rates, depths and
//! LFO settings 0 to 99, sensitivities 0 to 7), so a sound written down
//! for the original reads straight in, and [`bank`] reads the public
//! bulk-dump format for the owner's own sounds; none come with the app.
//! The level scale is the nonlinear one of the original: each step is
//! three quarters of a decibel near the top and the steps grow below
//! 20, so a modulator at 70 is quiet and one at 99 is wild. Envelope
//! segments that fall move at a steady rate in decibels; those that
//! rise move at a steady rate in amplitude, which is what gives the
//! attacks their snap. Modulation is phase modulation, as the original
//! does it, with a modulator at full level deviating the phase by four
//! pi, and feedback averaging the last two outputs so it stays smooth
//! up to its top, where a lone operator turns saw-like.
//!
//! Sources: the format's public documentation of its parameters and
//! bulk-dump layout, and published descriptions of how its envelopes,
//! level scale and feedback behave (the plan's sources note). No code
//! and no factory sounds from anyone.

use daw_engine::midi::{Event, Instrument, Kind};

use crate::dsp::{db, hz};
use crate::params::{Live, Param, Unit};
use crate::vintage::{Action, Allocator, Glide, GlideMode, Priority, VoiceSettings, soft};

pub mod bank;

/// How many operators a voice has.
pub const OPS: usize = 6;
/// Settings an operator has.
pub const OP: usize = 21;
/// The most voices there are.
pub const MAX_VOICES: usize = 16;
/// How many algorithms there are.
pub const ALGORITHMS: usize = 32;

const MODES: &[&str] = &["Ratio of the note", "Fixed Hz"];
const CURVES: &[&str] = &["−Linear", "−Exponential", "+Exponential", "+Linear"];
const LFO_WAVES: &[&str] = &[
    "Triangle",
    "Saw down",
    "Saw up",
    "Square",
    "Sine",
    "Sample and hold",
];
const PLAYING: &[&str] = &["Poly", "Mono", "Mono legato"];

/// An operator's settings by index within it; its first is
/// [`op_at`]`(o, 0)`.
pub mod op {
    pub const MODE: usize = 0;
    pub const COARSE: usize = 1;
    pub const FINE: usize = 2;
    pub const DETUNE: usize = 3;
    pub const LEVEL: usize = 4;
    /// Rates 1 to 4 follow each other, then levels 1 to 4.
    pub const RATE1: usize = 5;
    pub const LEVEL1: usize = 9;
    pub const BREAK: usize = 13;
    pub const LEFT_CURVE: usize = 14;
    pub const LEFT_DEPTH: usize = 15;
    pub const RIGHT_CURVE: usize = 16;
    pub const RIGHT_DEPTH: usize = 17;
    pub const RATE_SCALE: usize = 18;
    pub const VELOCITY: usize = 19;
    pub const AMP_MOD: usize = 20;
}

/// The index of operator `o`'s (from 0) setting `k` (one of [`op`]).
pub const fn op_at(o: usize, k: usize) -> usize {
    o * OP + k
}

/// The settings after the operators'; the order of [`params`].
pub mod at {
    use super::{OP, OPS};
    pub const ALGORITHM: usize = OPS * OP;
    pub const FEEDBACK: usize = ALGORITHM + 1;
    pub const OSC_SYNC: usize = ALGORITHM + 2;
    pub const LFO_WAVE: usize = ALGORITHM + 3;
    pub const LFO_SPEED: usize = ALGORITHM + 4;
    pub const LFO_DELAY: usize = ALGORITHM + 5;
    pub const LFO_PITCH: usize = ALGORITHM + 6;
    pub const LFO_AMP: usize = ALGORITHM + 7;
    pub const LFO_SYNC: usize = ALGORITHM + 8;
    pub const PITCH_MOD_SENS: usize = ALGORITHM + 9;
    /// Rates 1 to 4, then levels 1 to 4.
    pub const PITCH_RATE1: usize = ALGORITHM + 10;
    pub const PITCH_LEVEL1: usize = ALGORITHM + 14;
    pub const TRANSPOSE: usize = ALGORITHM + 18;
    pub const PLAYING: usize = ALGORITHM + 19;
    pub const VOICES: usize = ALGORITHM + 20;
    pub const GLIDE: usize = ALGORITHM + 21;
    pub const BEND: usize = ALGORITHM + 22;
    pub const WHEEL: usize = ALGORITHM + 23;
    pub const LEVEL: usize = ALGORITHM + 24;
    pub const COUNT: usize = ALGORITHM + 25;
}

/// The key of operator `o`'s (from 0) setting `k`, as projects name it.
pub fn op_key(o: usize, k: usize) -> String {
    let name = match k {
        op::MODE => "mode",
        op::COARSE => "coarse",
        op::FINE => "fine",
        op::DETUNE => "detune",
        op::LEVEL => "level",
        op::RATE1..=8 => return format!("op{}_rate{}", o + 1, k - op::RATE1 + 1),
        op::LEVEL1..=12 => return format!("op{}_level{}", o + 1, k - op::LEVEL1 + 1),
        op::BREAK => "break",
        op::LEFT_CURVE => "left_curve",
        op::LEFT_DEPTH => "left_depth",
        op::RIGHT_CURVE => "right_curve",
        op::RIGHT_DEPTH => "right_depth",
        op::RATE_SCALE => "rate_scale",
        op::VELOCITY => "velocity",
        _ => "amp_mod",
    };
    format!("op{}_{name}", o + 1)
}

/// The settings of operator `o` (from 0): the initial sound's first
/// operator is heard at full level, the rest are silent, with the
/// envelopes instant and the level held.
fn op_params(o: usize) -> Vec<Param> {
    use Unit::*;
    let n = o + 1;
    let k = |k: usize| op_key(o, k);
    let level = if o == 0 { 99.0 } else { 0.0 };
    let mut v = vec![
        Param::new(
            k(op::MODE),
            format!("Op {n} frequency"),
            0.0,
            1.0,
            0.0,
            Choice,
        )
        .choices(MODES),
        Param::new(
            k(op::COARSE),
            format!("Op {n} coarse"),
            0.0,
            31.0,
            1.0,
            Count,
        ),
        Param::new(k(op::FINE), format!("Op {n} fine"), 0.0, 99.0, 0.0, Count),
        Param::new(
            k(op::DETUNE),
            format!("Op {n} detune"),
            -7.0,
            7.0,
            0.0,
            Count,
        ),
        Param::new(
            k(op::LEVEL),
            format!("Op {n} level"),
            0.0,
            99.0,
            level,
            Count,
        ),
    ];
    for i in 0..4 {
        v.push(Param::new(
            k(op::RATE1 + i),
            format!("Op {n} rate {}", i + 1),
            0.0,
            99.0,
            99.0,
            Count,
        ));
    }
    for i in 0..4 {
        v.push(Param::new(
            k(op::LEVEL1 + i),
            format!("Op {n} level {}", i + 1),
            0.0,
            99.0,
            if i == 3 { 0.0 } else { 99.0 },
            Count,
        ));
    }
    v.extend([
        Param::new(
            k(op::BREAK),
            format!("Op {n} break point"),
            21.0,
            108.0,
            60.0,
            Note,
        ),
        Param::new(
            k(op::LEFT_CURVE),
            format!("Op {n} left curve"),
            0.0,
            3.0,
            0.0,
            Choice,
        )
        .choices(CURVES),
        Param::new(
            k(op::LEFT_DEPTH),
            format!("Op {n} left depth"),
            0.0,
            99.0,
            0.0,
            Count,
        ),
        Param::new(
            k(op::RIGHT_CURVE),
            format!("Op {n} right curve"),
            0.0,
            3.0,
            0.0,
            Choice,
        )
        .choices(CURVES),
        Param::new(
            k(op::RIGHT_DEPTH),
            format!("Op {n} right depth"),
            0.0,
            99.0,
            0.0,
            Count,
        ),
        Param::new(
            k(op::RATE_SCALE),
            format!("Op {n} rate scaling"),
            0.0,
            7.0,
            0.0,
            Count,
        ),
        Param::new(
            k(op::VELOCITY),
            format!("Op {n} velocity"),
            0.0,
            7.0,
            0.0,
            Count,
        ),
        Param::new(
            k(op::AMP_MOD),
            format!("Op {n} LFO to level"),
            0.0,
            3.0,
            0.0,
            Count,
        ),
    ]);
    v
}

/// The synth's settings, in the order of [`op_at`] and [`at`]. Grouped
/// for showing by [`GROUPS`].
pub fn params() -> Vec<Param> {
    use Unit::*;
    let mut v: Vec<Param> = (0..OPS).flat_map(op_params).collect();
    v.extend([
        Param::new("algorithm", "Algorithm", 1.0, ALGORITHMS as f64, 1.0, Count),
        Param::new("feedback", "Feedback", 0.0, 7.0, 0.0, Count),
        Param::new(
            "osc_sync",
            "Oscillators restart on each note",
            0.0,
            1.0,
            1.0,
            Switch,
        ),
        Param::new("lfo_wave", "LFO shape", 0.0, 5.0, 0.0, Choice).choices(LFO_WAVES),
        Param::new("lfo_speed", "LFO speed", 0.0, 99.0, 35.0, Count),
        Param::new("lfo_delay", "LFO delay", 0.0, 99.0, 0.0, Count),
        Param::new("lfo_pitch", "LFO to pitch", 0.0, 99.0, 0.0, Count),
        Param::new("lfo_amp", "LFO to level", 0.0, 99.0, 0.0, Count),
        Param::new(
            "lfo_sync",
            "LFO restarts on each note",
            0.0,
            1.0,
            1.0,
            Switch,
        ),
        Param::new(
            "pitch_mod_sens",
            "Pitch modulation sensitivity",
            0.0,
            7.0,
            3.0,
            Count,
        ),
    ]);
    for i in 0..4 {
        v.push(Param::new(
            format!("pitch_rate{}", i + 1),
            format!("Pitch rate {}", i + 1),
            0.0,
            99.0,
            99.0,
            Count,
        ));
    }
    for i in 0..4 {
        v.push(Param::new(
            format!("pitch_level{}", i + 1),
            format!("Pitch level {}", i + 1),
            0.0,
            99.0,
            50.0,
            Count,
        ));
    }
    v.extend([
        Param::new("transpose", "Transpose", -24.0, 24.0, 0.0, Semitones),
        Param::new("playing", "Playing", 0.0, 2.0, 0.0, Choice).choices(PLAYING),
        Param::new(
            "voices",
            "Voices",
            1.0,
            MAX_VOICES as f64,
            MAX_VOICES as f64,
            Count,
        ),
        Param::new("glide", "Glide time", 0.005, 3.0, 0.1, Seconds).log(),
        Param::new("bend", "Bend range", 1.0, 24.0, 2.0, Semitones),
        Param::new("wheel", "Mod wheel to LFO pitch", 0.0, 99.0, 50.0, Count),
        Param::new("level", "Level", -36.0, 6.0, -6.0, Db),
    ]);
    debug_assert_eq!(v.len(), at::COUNT);
    v
}

/// The settings in groups, for the controls: (heading, first, last).
pub const GROUPS: &[(&str, usize, usize)] = &[
    ("Algorithm", at::ALGORITHM, at::OSC_SYNC),
    ("Operator 1", op_at(0, 0), op_at(0, OP - 1)),
    ("Operator 2", op_at(1, 0), op_at(1, OP - 1)),
    ("Operator 3", op_at(2, 0), op_at(2, OP - 1)),
    ("Operator 4", op_at(3, 0), op_at(3, OP - 1)),
    ("Operator 5", op_at(4, 0), op_at(4, OP - 1)),
    ("Operator 6", op_at(5, 0), op_at(5, OP - 1)),
    ("LFO", at::LFO_WAVE, at::PITCH_MOD_SENS),
    ("Pitch envelope", at::PITCH_RATE1, at::PITCH_LEVEL1 + 3),
    ("Playing", at::TRANSPOSE, at::LEVEL),
];

/// An algorithm: for each operator, which operators modulate it (bit
/// `j` for operator `j`), which operators are heard, and the feedback
/// path from one operator into another (or itself).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Algorithm {
    pub modulators: [u8; OPS],
    pub carriers: u8,
    pub feedback_from: u8,
    pub feedback_to: u8,
}

const fn alg(modulators: [u8; OPS], carriers: u8, feedback_from: u8, feedback_to: u8) -> Algorithm {
    Algorithm {
        modulators,
        carriers,
        feedback_from,
        feedback_to,
    }
}

/// Operators as bits: `b(&[1, 2])` is operators 1 and 2.
const fn b(ops: &[u8]) -> u8 {
    let mut m = 0;
    let mut i = 0;
    while i < ops.len() {
        m |= 1 << (ops[i] - 1);
        i += 1;
    }
    m
}

/// The thirty-two algorithms, numbered from 1 as the original numbers
/// them, operators from 1 as well: `[op1, op2, …]` says what modulates
/// each.
pub const ALGORITHM_TABLE: [Algorithm; ALGORITHMS] = [
    alg([b(&[2]), 0, b(&[4]), b(&[5]), b(&[6]), 0], b(&[1, 3]), 6, 6),
    alg([b(&[2]), 0, b(&[4]), b(&[5]), b(&[6]), 0], b(&[1, 3]), 2, 2),
    alg([b(&[2]), b(&[3]), 0, b(&[5]), b(&[6]), 0], b(&[1, 4]), 6, 6),
    alg([b(&[2]), b(&[3]), 0, b(&[5]), b(&[6]), 0], b(&[1, 4]), 4, 6),
    alg([b(&[2]), 0, b(&[4]), 0, b(&[6]), 0], b(&[1, 3, 5]), 6, 6),
    alg([b(&[2]), 0, b(&[4]), 0, b(&[6]), 0], b(&[1, 3, 5]), 5, 6),
    alg([b(&[2]), 0, b(&[4, 5]), 0, b(&[6]), 0], b(&[1, 3]), 6, 6),
    alg([b(&[2]), 0, b(&[4, 5]), 0, b(&[6]), 0], b(&[1, 3]), 4, 4),
    alg([b(&[2]), 0, b(&[4, 5]), 0, b(&[6]), 0], b(&[1, 3]), 2, 2),
    alg([b(&[2]), b(&[3]), 0, b(&[5, 6]), 0, 0], b(&[1, 4]), 3, 3),
    alg([b(&[2]), b(&[3]), 0, b(&[5, 6]), 0, 0], b(&[1, 4]), 6, 6),
    alg([b(&[2]), 0, b(&[4, 5, 6]), 0, 0, 0], b(&[1, 3]), 2, 2),
    alg([b(&[2]), 0, b(&[4, 5, 6]), 0, 0, 0], b(&[1, 3]), 6, 6),
    alg([b(&[2]), 0, b(&[4]), b(&[5, 6]), 0, 0], b(&[1, 3]), 6, 6),
    alg([b(&[2]), 0, b(&[4]), b(&[5, 6]), 0, 0], b(&[1, 3]), 2, 2),
    alg([b(&[2, 3, 5]), 0, b(&[4]), 0, b(&[6]), 0], b(&[1]), 6, 6),
    alg([b(&[2, 3, 5]), 0, b(&[4]), 0, b(&[6]), 0], b(&[1]), 2, 2),
    alg([b(&[2, 3, 4]), 0, 0, b(&[5]), b(&[6]), 0], b(&[1]), 3, 3),
    alg(
        [b(&[2]), b(&[3]), 0, b(&[6]), b(&[6]), 0],
        b(&[1, 4, 5]),
        6,
        6,
    ),
    alg([b(&[3]), b(&[3]), 0, b(&[5, 6]), 0, 0], b(&[1, 2, 4]), 3, 3),
    alg(
        [b(&[3]), b(&[3]), 0, b(&[6]), b(&[6]), 0],
        b(&[1, 2, 4, 5]),
        3,
        3,
    ),
    alg(
        [b(&[2]), 0, b(&[6]), b(&[6]), b(&[6]), 0],
        b(&[1, 3, 4, 5]),
        6,
        6,
    ),
    alg([0, b(&[3]), 0, b(&[6]), b(&[6]), 0], b(&[1, 2, 4, 5]), 6, 6),
    alg(
        [0, 0, b(&[6]), b(&[6]), b(&[6]), 0],
        b(&[1, 2, 3, 4, 5]),
        6,
        6,
    ),
    alg([0, 0, 0, b(&[6]), b(&[6]), 0], b(&[1, 2, 3, 4, 5]), 6, 6),
    alg([0, b(&[3]), 0, b(&[5, 6]), 0, 0], b(&[1, 2, 4]), 6, 6),
    alg([0, b(&[3]), 0, b(&[5, 6]), 0, 0], b(&[1, 2, 4]), 3, 3),
    alg([b(&[2]), 0, b(&[4]), b(&[5]), 0, 0], b(&[1, 3, 6]), 5, 5),
    alg([0, 0, b(&[4]), 0, b(&[6]), 0], b(&[1, 2, 3, 5]), 6, 6),
    alg([0, 0, b(&[4]), b(&[5]), 0, 0], b(&[1, 2, 3, 6]), 5, 5),
    alg([0, 0, 0, 0, b(&[6]), 0], b(&[1, 2, 3, 4, 5]), 6, 6),
    alg([0, 0, 0, 0, 0, 0], b(&[1, 2, 3, 4, 5, 6]), 6, 6),
];

/// Algorithm `n` (from 1); out of range is 1.
pub fn algorithm(n: f32) -> &'static Algorithm {
    let i = (n as usize).clamp(1, ALGORITHMS) - 1;
    &ALGORITHM_TABLE[i]
}

/// The sounds it makes from its settings.
pub fn sounds() -> Vec<crate::Sound> {
    use crate::Sound;
    vec![
        Sound {
            name: "Tine piano",
            settings: &[
                ("algorithm", 5.0),
                ("feedback", 6.0),
                // Three pairs: the body, the tine an octave up with its
                // bright fourteenth harmonic, and a soft lower pair.
                ("op1_level", 99.0),
                ("op1_rate1", 95.0),
                ("op1_rate2", 25.0),
                ("op1_rate3", 25.0),
                ("op1_rate4", 60.0),
                ("op1_level2", 80.0),
                ("op1_level3", 0.0),
                ("op1_velocity", 2.0),
                ("op1_rate_scale", 2.0),
                ("op2_coarse", 14.0),
                ("op2_level", 72.0),
                ("op2_rate1", 95.0),
                ("op2_rate2", 50.0),
                ("op2_rate3", 35.0),
                ("op2_rate4", 78.0),
                ("op2_level2", 50.0),
                ("op2_level3", 0.0),
                ("op2_velocity", 7.0),
                ("op2_rate_scale", 3.0),
                ("op3_level", 99.0),
                ("op3_rate1", 95.0),
                ("op3_rate2", 20.0),
                ("op3_rate3", 20.0),
                ("op3_rate4", 50.0),
                ("op3_level2", 95.0),
                ("op3_level3", 0.0),
                ("op3_velocity", 2.0),
                ("op3_rate_scale", 2.0),
                ("op4_level", 89.0),
                ("op4_rate1", 95.0),
                ("op4_rate2", 29.0),
                ("op4_rate3", 20.0),
                ("op4_rate4", 50.0),
                ("op4_level2", 95.0),
                ("op4_level3", 0.0),
                ("op4_velocity", 6.0),
                ("op4_rate_scale", 2.0),
                ("op4_break", 60.0),
                ("op4_right_curve", 0.0),
                ("op4_right_depth", 30.0),
                ("op5_level", 99.0),
                ("op5_rate1", 95.0),
                ("op5_rate2", 20.0),
                ("op5_rate3", 20.0),
                ("op5_rate4", 50.0),
                ("op5_level2", 95.0),
                ("op5_level3", 0.0),
                ("op5_velocity", 2.0),
                ("op5_rate_scale", 2.0),
                ("op6_level", 79.0),
                ("op6_rate1", 95.0),
                ("op6_rate2", 29.0),
                ("op6_rate3", 20.0),
                ("op6_rate4", 50.0),
                ("op6_level2", 95.0),
                ("op6_level3", 0.0),
                ("op6_velocity", 6.0),
                ("op6_rate_scale", 2.0),
                ("op6_detune", 7.0),
                ("lfo_speed", 34.0),
                ("lfo_amp", 10.0),
                ("op1_amp_mod", 1.0),
                ("op3_amp_mod", 1.0),
                ("op5_amp_mod", 1.0),
                ("transpose", 0.0),
            ],
        },
        Sound {
            name: "Tubular bells",
            settings: &[
                ("algorithm", 5.0),
                ("feedback", 0.0),
                // Carriers and modulators in the inharmonic ratios of a
                // struck bar: 1 against 3.5, and a hum an octave under.
                ("op1_level", 99.0),
                ("op1_rate1", 95.0),
                ("op1_rate2", 20.0),
                ("op1_rate3", 20.0),
                ("op1_rate4", 40.0),
                ("op1_level2", 70.0),
                ("op1_level3", 0.0),
                ("op1_velocity", 3.0),
                ("op2_coarse", 3.0),
                ("op2_fine", 50.0),
                ("op2_level", 70.0),
                ("op2_rate1", 95.0),
                ("op2_rate2", 30.0),
                ("op2_rate3", 30.0),
                ("op2_rate4", 60.0),
                ("op2_level2", 60.0),
                ("op2_level3", 0.0),
                ("op2_velocity", 5.0),
                ("op3_coarse", 2.0),
                ("op3_level", 85.0),
                ("op3_rate1", 95.0),
                ("op3_rate2", 24.0),
                ("op3_rate3", 24.0),
                ("op3_rate4", 45.0),
                ("op3_level2", 70.0),
                ("op3_level3", 0.0),
                ("op3_velocity", 3.0),
                ("op4_coarse", 7.0),
                ("op4_level", 66.0),
                ("op4_rate1", 95.0),
                ("op4_rate2", 40.0),
                ("op4_rate3", 40.0),
                ("op4_rate4", 70.0),
                ("op4_level2", 50.0),
                ("op4_level3", 0.0),
                ("op4_velocity", 6.0),
                ("op5_coarse", 0.0),
                ("op5_level", 80.0),
                ("op5_rate1", 90.0),
                ("op5_rate2", 15.0),
                ("op5_rate3", 15.0),
                ("op5_rate4", 40.0),
                ("op5_level2", 80.0),
                ("op5_level3", 0.0),
                ("op6_coarse", 1.0),
                ("op6_fine", 73.0),
                ("op6_level", 60.0),
                ("op6_rate1", 95.0),
                ("op6_rate2", 35.0),
                ("op6_rate3", 35.0),
                ("op6_rate4", 60.0),
                ("op6_level2", 50.0),
                ("op6_level3", 0.0),
            ],
        },
        Sound {
            name: "Solid bass",
            settings: &[
                ("algorithm", 1.0),
                ("feedback", 7.0),
                // One pair with a fast-decaying bite from the
                // modulator, and a second pair an octave under for
                // weight.
                ("op1_level", 99.0),
                ("op1_rate1", 99.0),
                ("op1_rate2", 60.0),
                ("op1_rate3", 40.0),
                ("op1_rate4", 80.0),
                ("op1_level2", 90.0),
                ("op1_level3", 75.0),
                ("op1_velocity", 2.0),
                ("op2_level", 82.0),
                ("op2_rate1", 99.0),
                ("op2_rate2", 70.0),
                ("op2_rate3", 50.0),
                ("op2_rate4", 85.0),
                ("op2_level2", 55.0),
                ("op2_level3", 40.0),
                ("op2_velocity", 6.0),
                ("op2_rate_scale", 3.0),
                ("op3_coarse", 0.0),
                ("op3_level", 92.0),
                ("op3_rate1", 99.0),
                ("op3_rate2", 50.0),
                ("op3_rate3", 40.0),
                ("op3_rate4", 80.0),
                ("op3_level2", 95.0),
                ("op3_level3", 80.0),
                ("op4_coarse", 0.0),
                ("op4_level", 70.0),
                ("op4_rate1", 99.0),
                ("op4_rate2", 75.0),
                ("op4_rate3", 60.0),
                ("op4_rate4", 85.0),
                ("op4_level2", 40.0),
                ("op4_level3", 20.0),
                ("op4_velocity", 5.0),
                ("op5_level", 0.0),
                ("op6_coarse", 1.0),
                ("op6_level", 75.0),
                ("op6_rate1", 99.0),
                ("op6_rate2", 80.0),
                ("op6_rate3", 60.0),
                ("op6_rate4", 90.0),
                ("op6_level2", 30.0),
                ("op6_level3", 0.0),
                ("op6_velocity", 5.0),
                ("playing", 1.0),
                ("glide", 0.03),
            ],
        },
        Sound {
            name: "Brass section",
            settings: &[
                ("algorithm", 18.0),
                ("feedback", 5.0),
                // One carrier fed by three modulators, swelling in as
                // a brass player's breath does.
                ("op1_level", 99.0),
                ("op1_rate1", 72.0),
                ("op1_rate2", 50.0),
                ("op1_rate3", 30.0),
                ("op1_rate4", 60.0),
                ("op1_level2", 92.0),
                ("op1_level3", 85.0),
                ("op1_velocity", 3.0),
                ("op2_level", 78.0),
                ("op2_rate1", 60.0),
                ("op2_rate2", 40.0),
                ("op2_rate3", 30.0),
                ("op2_rate4", 60.0),
                ("op2_level2", 85.0),
                ("op2_level3", 75.0),
                ("op2_velocity", 5.0),
                ("op2_detune", -3.0),
                ("op3_level", 76.0),
                ("op3_rate1", 55.0),
                ("op3_rate2", 40.0),
                ("op3_rate3", 30.0),
                ("op3_rate4", 60.0),
                ("op3_level2", 80.0),
                ("op3_level3", 70.0),
                ("op3_velocity", 5.0),
                ("op3_detune", 3.0),
                ("op4_coarse", 2.0),
                ("op4_level", 60.0),
                ("op4_rate1", 50.0),
                ("op4_rate2", 40.0),
                ("op4_rate3", 30.0),
                ("op4_rate4", 60.0),
                ("op4_level2", 70.0),
                ("op4_level3", 60.0),
                ("op4_velocity", 6.0),
                ("op5_level", 70.0),
                ("op5_rate1", 65.0),
                ("op5_rate2", 40.0),
                ("op5_rate3", 30.0),
                ("op5_rate4", 60.0),
                ("op5_level2", 80.0),
                ("op5_level3", 70.0),
                ("op6_level", 55.0),
                ("op6_rate1", 65.0),
                ("op6_rate2", 40.0),
                ("op6_rate3", 30.0),
                ("op6_rate4", 60.0),
                ("op6_level2", 80.0),
                ("op6_level3", 70.0),
                ("op6_break", 60.0),
                ("op6_right_curve", 0.0),
                ("op6_right_depth", 40.0),
                ("lfo_speed", 30.0),
                ("lfo_delay", 55.0),
                ("lfo_pitch", 20.0),
                ("pitch_mod_sens", 2.0),
                ("pitch_rate1", 90.0),
                ("pitch_level4", 46.0),
                ("pitch_level1", 50.0),
            ],
        },
        Sound {
            name: "Pluck",
            settings: &[
                ("algorithm", 8.0),
                ("feedback", 6.0),
                // A plucked string: a bright stack that dies to a
                // plain tone, quicker up the keyboard.
                ("op1_level", 99.0),
                ("op1_rate1", 99.0),
                ("op1_rate2", 35.0),
                ("op1_rate3", 35.0),
                ("op1_rate4", 70.0),
                ("op1_level2", 70.0),
                ("op1_level3", 0.0),
                ("op1_rate_scale", 3.0),
                ("op1_velocity", 2.0),
                ("op2_coarse", 2.0),
                ("op2_level", 72.0),
                ("op2_rate1", 99.0),
                ("op2_rate2", 60.0),
                ("op2_rate3", 40.0),
                ("op2_rate4", 80.0),
                ("op2_level2", 40.0),
                ("op2_level3", 0.0),
                ("op2_rate_scale", 4.0),
                ("op2_velocity", 6.0),
                ("op3_level", 92.0),
                ("op3_rate1", 99.0),
                ("op3_rate2", 40.0),
                ("op3_rate3", 35.0),
                ("op3_rate4", 70.0),
                ("op3_level2", 60.0),
                ("op3_level3", 0.0),
                ("op3_rate_scale", 3.0),
                ("op3_velocity", 2.0),
                ("op4_coarse", 3.0),
                ("op4_level", 68.0),
                ("op4_rate1", 99.0),
                ("op4_rate2", 65.0),
                ("op4_rate3", 45.0),
                ("op4_rate4", 85.0),
                ("op4_level2", 30.0),
                ("op4_level3", 0.0),
                ("op4_rate_scale", 4.0),
                ("op4_velocity", 7.0),
                ("op5_coarse", 1.0),
                ("op5_level", 60.0),
                ("op5_rate1", 99.0),
                ("op5_rate2", 70.0),
                ("op5_rate3", 50.0),
                ("op5_rate4", 85.0),
                ("op5_level2", 30.0),
                ("op5_level3", 0.0),
                ("op5_velocity", 6.0),
                ("op6_level", 0.0),
            ],
        },
        Sound {
            name: "Glass pad",
            settings: &[
                ("algorithm", 19.0),
                ("feedback", 3.0),
                // Slow-rising carriers in octaves with a fifth above,
                // the modulators gently detuned so the pad moves.
                ("op1_level", 92.0),
                ("op1_rate1", 40.0),
                ("op1_rate2", 30.0),
                ("op1_rate3", 30.0),
                ("op1_rate4", 35.0),
                ("op1_level2", 90.0),
                ("op1_level3", 85.0),
                ("op2_level", 60.0),
                ("op2_rate1", 35.0),
                ("op2_rate2", 30.0),
                ("op2_rate3", 30.0),
                ("op2_rate4", 40.0),
                ("op2_level2", 70.0),
                ("op2_level3", 60.0),
                ("op2_detune", 4.0),
                ("op3_coarse", 2.0),
                ("op3_level", 55.0),
                ("op3_rate1", 30.0),
                ("op3_rate2", 30.0),
                ("op3_rate3", 30.0),
                ("op3_rate4", 40.0),
                ("op3_level2", 70.0),
                ("op3_level3", 60.0),
                ("op3_detune", -4.0),
                ("op4_coarse", 2.0),
                ("op4_level", 85.0),
                ("op4_rate1", 38.0),
                ("op4_rate2", 30.0),
                ("op4_rate3", 30.0),
                ("op4_rate4", 35.0),
                ("op4_level2", 85.0),
                ("op4_level3", 80.0),
                ("op4_detune", 3.0),
                ("op5_coarse", 3.0),
                ("op5_level", 72.0),
                ("op5_rate1", 36.0),
                ("op5_rate2", 30.0),
                ("op5_rate3", 30.0),
                ("op5_rate4", 35.0),
                ("op5_level2", 80.0),
                ("op5_level3", 75.0),
                ("op5_detune", -3.0),
                ("op6_coarse", 1.0),
                ("op6_level", 62.0),
                ("op6_rate1", 30.0),
                ("op6_rate2", 30.0),
                ("op6_rate3", 30.0),
                ("op6_rate4", 40.0),
                ("op6_level2", 75.0),
                ("op6_level3", 65.0),
                ("lfo_speed", 28.0),
                ("lfo_delay", 40.0),
                ("lfo_pitch", 25.0),
                ("lfo_amp", 30.0),
                ("op4_amp_mod", 1.0),
                ("op5_amp_mod", 2.0),
                ("pitch_mod_sens", 2.0),
            ],
        },
        Sound {
            name: "Drawbar organ",
            settings: &[
                ("algorithm", 32.0),
                ("feedback", 0.0),
                // Six sines as drawbars: the fundamental, the octave,
                // the twelfth, the fifteenth, the sub and the
                // seventeenth, held while the key is down.
                ("op1_level", 99.0),
                ("op1_level3", 99.0),
                ("op1_rate4", 85.0),
                ("op2_coarse", 2.0),
                ("op2_level", 90.0),
                ("op2_level3", 99.0),
                ("op2_rate4", 85.0),
                ("op3_coarse", 3.0),
                ("op3_level", 84.0),
                ("op3_level3", 99.0),
                ("op3_rate4", 85.0),
                ("op4_coarse", 4.0),
                ("op4_level", 76.0),
                ("op4_level3", 99.0),
                ("op4_rate4", 85.0),
                ("op5_coarse", 0.0),
                ("op5_level", 88.0),
                ("op5_level3", 99.0),
                ("op5_rate4", 85.0),
                ("op6_coarse", 5.0),
                ("op6_level", 60.0),
                ("op6_level3", 99.0),
                ("op6_rate4", 85.0),
                ("lfo_wave", 4.0),
                ("lfo_speed", 60.0),
                ("lfo_amp", 35.0),
                ("op2_amp_mod", 1.0),
                ("op3_amp_mod", 1.0),
                ("op6_amp_mod", 2.0),
                ("level", -10.0),
            ],
        },
    ]
}

/// Settings are read this often: a third of a millisecond at 48 kHz.
const CHUNK: usize = 16;

/// A modulator at full level deviates the carrier's phase this far, in
/// radians.
const INDEX: f32 = 4.0 * std::f32::consts::PI;

/// Feedback at 7 deviates the operator's phase this far, in radians,
/// times the average of its last two outputs: enough for a lone
/// carrier to turn saw-like without the loop running away into noise.
/// Each step down halves it.
const FEEDBACK: f32 = 1.5;

/// The sine, by table: `SINE[i]` is sin(i / 2048 of a turn).
const SINE_BITS: u32 = 11;
const SINE_N: usize = 1 << SINE_BITS;

struct Sine([f32; SINE_N + 1]);

impl Sine {
    fn new() -> Sine {
        let mut t = [0.0; SINE_N + 1];
        for (i, v) in t.iter_mut().enumerate() {
            *v = (i as f64 / SINE_N as f64 * std::f64::consts::TAU).sin() as f32;
        }
        Sine(t)
    }

    /// sin(2π phase) for any phase (in turns), interpolated.
    #[inline]
    fn at(&self, phase: f32) -> f32 {
        let p = (phase - phase.floor()) * SINE_N as f32;
        let i = p as usize;
        let f = p - i as f32;
        let a = self.0[i.min(SINE_N)];
        let b = self.0[(i + 1).min(SINE_N)];
        a + (b - a) * f
    }
}

/// The level scale to amplitude: units 0 to 99 (with fractions), each
/// step three quarters of a decibel near the top, steeper under 20,
/// down to silence at 0. Above 99 keeps climbing (velocity and scaling
/// can push a level past the top, as on the original).
#[inline]
pub fn level_to_amp(units: f32) -> f32 {
    if units <= 0.0 {
        return 0.0;
    }
    // The steps under 20 cover what twenty 0.75 dB steps would, times
    // 2.35: a level of 20 is 47 units' worth below... so it goes from
    // -94 dB at 1 up to -60 dB at 20, then 0.75 dB a step to 0 dB at 99.
    let eq = if units >= 20.0 {
        units + 27.0
    } else {
        units * 2.35
    };
    (0.125 * (eq - 126.0)).exp2()
}

/// The inverse of [`level_to_amp`].
#[cfg(test)]
fn amp_to_level(amp: f32) -> f32 {
    if amp <= 0.0 {
        return 0.0;
    }
    let eq = amp.log2() / 0.125 + 126.0;
    if eq >= 47.0 { eq - 27.0 } else { eq / 2.35 }
}

/// Seconds an envelope segment takes to cross the whole range at
/// `rate` (0 to 99): a rate of 99 is near-instant, 50 about half a
/// second, 0 over a minute.
#[inline]
fn rate_seconds(rate: f32) -> f32 {
    0.004 * ((99.0 - rate.clamp(0.0, 99.0)) / 7.0).exp2()
}

/// A four-rate, four-level envelope on the level scale, 0 to 99: from
/// the fourth level it moves to the first, second and third in turn,
/// holds the third while the key is down, and on release moves to the
/// fourth. Falling segments move at a steady rate in decibels (the
/// level scale), rising ones at a steady rate in amplitude. It runs in
/// amplitude, so a sample costs an add or a multiply and nothing
/// transcendental.
#[derive(Clone, Copy, Debug, Default)]
struct Env {
    /// 0 to 2 while held, 3 releasing, 4 idle.
    segment: u8,
    pub amp: f32,
}

/// An envelope's per-sample steps for one voice: the target
/// amplitudes, the amplitude added a sample on a rise, and the factors
/// a fall multiplies by (above and below level 20, where the scale
/// steepens).
#[derive(Clone, Copy, Debug, Default)]
struct EnvSteps {
    target: [f32; 4],
    up: [f32; 4],
    down: [(f32, f32); 4],
}

/// The amplitude at level 20, under which the scale's steps grow.
const AMP20: f32 = 0.001_066; // level_to_amp(20.0)

impl EnvSteps {
    /// For levels and rates 0 to 99 (the rates already scaled) at a
    /// sample rate.
    fn new(levels: [f32; 4], rates: [f32; 4], rate: f32) -> EnvSteps {
        let mut e = EnvSteps::default();
        for i in 0..4 {
            let samples = (rate_seconds(rates[i]) * rate).max(1.0);
            e.target[i] = level_to_amp(levels[i]);
            e.up[i] = 1.0 / samples;
            let units = 99.0 / samples;
            e.down[i] = ((-0.125 * units).exp2(), (-0.125 * 2.35 * units).exp2());
        }
        e
    }
}

impl Env {
    fn trigger(&mut self, e: &EnvSteps) {
        self.amp = e.target[3];
        self.segment = 0;
    }

    fn release(&mut self) {
        if self.segment < 3 {
            self.segment = 3;
        }
    }

    fn idle(&self) -> bool {
        self.segment >= 4
    }

    /// Where it is on the level scale.
    #[cfg(test)]
    fn level(&self) -> f32 {
        amp_to_level(self.amp)
    }

    /// One sample on; the amplitude.
    #[inline]
    fn next(&mut self, e: &EnvSteps) -> f32 {
        let seg = self.segment as usize;
        if seg >= 4 {
            return self.amp;
        }
        let target = e.target[seg];
        let arrived = if target > self.amp {
            self.amp += e.up[seg];
            self.amp >= target
        } else {
            let (above, below) = e.down[seg];
            self.amp *= if self.amp > AMP20 { above } else { below };
            // Silence is never reached by multiplying: close enough is
            // -100 dB.
            self.amp <= target || self.amp < 1e-5
        };
        if arrived {
            self.amp = target;
            if seg == 3 {
                self.segment = 4;
            } else if seg != 2 {
                self.segment += 1;
            }
        }
        self.amp
    }
}

/// The pitch envelope: four rates and four levels on the 0 to 99 scale
/// (50 at rest), moving at a steady rate in level.
#[derive(Clone, Copy, Debug, Default)]
struct PitchEnv {
    segment: u8,
    level: f32,
}

impl PitchEnv {
    fn trigger(&mut self, levels: &[f32; 4]) {
        self.level = levels[3];
        self.segment = 0;
    }

    fn release(&mut self) {
        if self.segment < 3 {
            self.segment = 3;
        }
    }

    #[inline]
    fn next(&mut self, levels: &[f32; 4], step: &[f32; 4]) -> f32 {
        let seg = self.segment as usize;
        if seg >= 4 {
            return self.level;
        }
        let target = levels[seg];
        let d = target - self.level;
        if d.abs() <= step[seg] {
            self.level = target;
            if seg == 3 {
                self.segment = 4;
            } else if seg != 2 {
                self.segment += 1;
            }
        } else {
            self.level += step[seg].copysign(d);
        }
        self.level
    }
}

/// An operator as a voice plays it.
#[derive(Clone, Copy, Debug, Default)]
struct OpState {
    /// In turns.
    phase: f32,
    dt: f32,
    env: Env,
    steps: EnvSteps,
    /// Its output this sample and the one before, for feedback.
    out: f32,
    prev: f32,
    /// What the envelope's amplitude is multiplied by: the output
    /// level, level scaling, velocity and the LFO; set once a chunk.
    gain: f32,
}

/// One voice.
struct Voice {
    ops: [OpState; OPS],
    pitch_env: PitchEnv,
    pitch_levels: [f32; 4],
    /// Per segment, how far the pitch envelope moves a sample.
    pitch_step: [f32; 4],
    on: bool,
    /// Whether it has been released and the carriers have all gone idle:
    /// a short fade then takes it out.
    fading: f32,
    note: u8,
    velocity: u8,
    pitch: f32,
    target: f32,
}

impl Voice {
    fn new() -> Voice {
        Voice {
            ops: [OpState::default(); OPS],
            pitch_env: PitchEnv::default(),
            pitch_levels: [50.0; 4],
            pitch_step: [0.0; 4],
            on: false,
            fading: 1.0,
            note: 60,
            velocity: 100,
            pitch: 60.0,
            target: 60.0,
        }
    }
}

/// The LFO's shape at `phase` (in turns), −1 to 1; sample-and-hold
/// takes the value held.
#[inline]
fn lfo_wave(wave: u32, phase: f32, held: f32) -> f32 {
    match wave {
        0 => 1.0 - 4.0 * (phase - 0.5).abs(),
        1 => 1.0 - 2.0 * phase,
        2 => 2.0 * phase - 1.0,
        3 => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        4 => (phase * std::f32::consts::TAU).sin(),
        _ => held,
    }
}

/// The LFO's speed setting as Hz: 0 is a slow sweep every sixteen
/// seconds, 99 is 47 Hz, 35 about 3 Hz.
pub fn lfo_hz(speed: f32) -> f32 {
    0.0626 * 750f32.powf((speed.clamp(0.0, 99.0) / 99.0).sqrt())
}

/// The LFO's delay setting as seconds before it starts to fade in.
fn lfo_delay_seconds(delay: f32) -> f32 {
    let d = delay.clamp(0.0, 99.0) / 99.0;
    d * d * 6.0
}

/// The pitch modulation sensitivity's range in semitones at full depth.
fn pitch_mod_range(sens: f32) -> f32 {
    match sens as u32 {
        0 => 0.0,
        s => 12.0 * (s.min(7) as f32 - 7.0).exp2(),
    }
}

/// A pitch envelope level (0 to 99, 50 at rest) as semitones: fine near
/// the middle, four octaves at the ends.
fn pitch_level_semitones(level: f32) -> f32 {
    let s = (level - 50.0) / 50.0;
    48.0 * s * s.abs()
}

/// The synth on the audio thread.
pub struct LuckySeven {
    params: Live,
    rate: f32,
    sine: Sine,
    alloc: Allocator,
    voices: Vec<Voice>,
    bend: f32,
    wheel: f32,
    pressure: f32,
    keys_down: u32,
    lfo_phase: f32,
    lfo_held: f32,
    lfo_since: f32,
    noise: u32,
    actions: Vec<Action>,
}

impl LuckySeven {
    pub fn new(rate: u32, params: impl Into<Live>) -> LuckySeven {
        LuckySeven {
            params: params.into(),
            rate: rate.max(1) as f32,
            sine: Sine::new(),
            alloc: Allocator::new(MAX_VOICES),
            voices: (0..MAX_VOICES).map(|_| Voice::new()).collect(),
            bend: 0.0,
            wheel: 0.0,
            pressure: 0.0,
            keys_down: 0,
            lfo_phase: 0.0,
            lfo_held: 0.0,
            lfo_since: 0.0,
            noise: 0x1983_0707,
            actions: Vec::with_capacity(2 * MAX_VOICES),
        }
    }

    fn settings(&self) -> VoiceSettings {
        let voices = (self.params.read(at::VOICES) as usize).clamp(1, MAX_VOICES);
        match self.params.read(at::PLAYING) as u32 {
            1 | 2 => VoiceSettings {
                notes: 1,
                unison: 1,
                priority: Priority::Last,
                legato: self.params.read(at::PLAYING) as u32 == 2,
            },
            _ => VoiceSettings {
                notes: voices,
                unison: 1,
                priority: Priority::Last,
                legato: false,
            },
        }
    }

    /// The allocator is sized to the voices setting whenever every
    /// voice is quiet.
    fn resize(&mut self) {
        let want = (self.params.read(at::VOICES) as usize).clamp(1, MAX_VOICES);
        if self.alloc.voices() != want && self.voices.iter().all(|v| !v.on) {
            self.alloc = Allocator::new(want);
        }
    }

    /// Starts voice `voice` on `note`: the envelopes from their fourth
    /// level, the operators' phases from zero when the sync switch is
    /// on.
    fn start(&mut self, voice: usize, note: u8, velocity: u8, fresh: bool) {
        let sync = self.params.read(at::OSC_SYNC) >= 0.5;
        let v = &mut self.voices[voice];
        v.note = note;
        v.velocity = velocity.clamp(1, 127);
        v.target = note as f32;
        if fresh || self.params.read(at::PLAYING) as u32 == 0 {
            v.pitch = v.target;
        }
        v.fading = 1.0;
        let rate = self.rate;
        for (o, s) in v.ops.iter_mut().enumerate() {
            let levels = std::array::from_fn(|i| self.params.read(op_at(o, op::LEVEL1 + i)));
            let rates = std::array::from_fn(|i| self.params.read(op_at(o, op::RATE1 + i)));
            s.steps = EnvSteps::new(levels, rates, rate);
            s.env.trigger(&s.steps);
            if sync || fresh {
                s.phase = 0.0;
                s.out = 0.0;
                s.prev = 0.0;
            }
        }
        for i in 0..4 {
            v.pitch_levels[i] = self.params.read(at::PITCH_LEVEL1 + i);
        }
        v.pitch_env.trigger(&v.pitch_levels);
        v.on = true;
    }

    fn apply(&mut self) {
        for i in 0..self.actions.len() {
            match self.actions[i] {
                Action::Start {
                    voice,
                    note,
                    velocity,
                    fresh,
                } => self.start(voice, note, velocity, fresh),
                Action::Retune { voice, note } => {
                    let v = &mut self.voices[voice];
                    v.note = note;
                    v.target = note as f32;
                }
                Action::Release { voice } => {
                    let v = &mut self.voices[voice];
                    for s in &mut v.ops {
                        s.env.release();
                    }
                    v.pitch_env.release();
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
                if self.keys_down == 0 && self.params.read(at::LFO_SYNC) >= 0.5 {
                    self.lfo_phase = 0.0;
                    self.lfo_since = 0.0;
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
            Kind::Pressure(p) => self.pressure = p as f32 / 127.0,
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

    fn chunk(&mut self, from: usize, to: usize, left: &mut [f32], right: &mut [f32]) {
        let count = to - from;
        let n = count as f32;
        let r = self.rate;
        let p = &self.params;

        // The LFO, shared by the voices: its shape, delayed fade-in,
        // and depths to pitch and level.
        let wave = p.read(at::LFO_WAVE) as u32;
        let lfo_dt = lfo_hz(p.read(at::LFO_SPEED)) / r;
        let delay = lfo_delay_seconds(p.read(at::LFO_DELAY));
        self.lfo_since += n / r;
        let fade = if delay <= 0.0 {
            1.0
        } else {
            ((self.lfo_since - delay) / (0.5 * delay + 0.05)).clamp(0.0, 1.0)
        };
        let before = self.lfo_phase;
        self.lfo_phase += lfo_dt * n;
        if self.lfo_phase >= 1.0 {
            self.lfo_phase -= self.lfo_phase.floor();
            // A new random level each cycle for sample-and-hold.
            self.noise ^= self.noise << 13;
            self.noise ^= self.noise >> 17;
            self.noise ^= self.noise << 5;
            self.lfo_held = (self.noise >> 8) as f32 / 8_388_608.0 - 1.0;
        }
        let lfo = lfo_wave(wave, before, self.lfo_held) * fade;
        let pms = pitch_mod_range(p.read(at::PITCH_MOD_SENS));
        let wheel = self.wheel.max(self.pressure) * p.read(at::WHEEL) / 99.0;
        let lfo_pitch = pms * (p.read(at::LFO_PITCH) / 99.0 + wheel).min(1.0) * lfo;
        // To level: at the LFO's top nothing is taken off, at its
        // bottom the operator's sensitivity's worth.
        let amd = p.read(at::LFO_AMP) / 99.0 * (1.0 - lfo) * 0.5;

        if self.voices.iter().all(|v| !v.on) {
            return;
        }

        let algorithm = *algorithm(p.read(at::ALGORITHM));
        let fb = p.read(at::FEEDBACK);
        let fb_gain = if fb < 0.5 {
            0.0
        } else {
            FEEDBACK * (fb.min(7.0) - 7.0).exp2()
        };
        let transpose = p.read(at::TRANSPOSE);
        let bend = self.bend * p.read(at::BEND);
        let gain = db(p.read(at::LEVEL)) * 0.45;
        let glide = Glide {
            mode: GlideMode::Time,
            seconds: p.read(at::GLIDE),
        };
        let pitch_step: [f32; 4] = std::array::from_fn(|i| {
            99.0 / (rate_seconds(p.read(at::PITCH_RATE1 + i)) * r).max(1.0)
        });
        // What each operator is set to, read once a chunk for all the
        // voices.
        struct OpNow {
            fixed: bool,
            ratio: f32,
            fixed_hz: f32,
            level: f32,
            rates: [f32; 4],
            break_note: f32,
            left: (u32, f32),
            right: (u32, f32),
            rate_scale: f32,
            velocity: f32,
            amp_mod: f32,
        }
        let ops: [OpNow; OPS] = std::array::from_fn(|o| {
            let coarse = p.read(op_at(o, op::COARSE)).clamp(0.0, 31.0);
            let fine = p.read(op_at(o, op::FINE)).clamp(0.0, 99.0);
            let detune = p.read(op_at(o, op::DETUNE)).clamp(-7.0, 7.0);
            let fixed = p.read(op_at(o, op::MODE)) >= 0.5;
            // In ratio mode coarse 0 is a half, else the whole
            // number, and fine adds hundredths of it; fixed, coarse
            // picks 1, 10, 100 or 1000 Hz and fine moves up to a
            // decade in a hundred steps.
            let base = if coarse < 0.5 { 0.5 } else { coarse.round() };
            let ratio = base * (1.0 + fine / 100.0) * (1.0 + detune * 0.0022);
            let fixed_hz = 10f32.powi((coarse.round() as i32).rem_euclid(4))
                * 10f32.powf(fine / 100.0)
                + detune * 0.05;
            OpNow {
                fixed,
                ratio,
                fixed_hz,
                level: p.read(op_at(o, op::LEVEL)),
                rates: [
                    p.read(op_at(o, op::RATE1)),
                    p.read(op_at(o, op::RATE1 + 1)),
                    p.read(op_at(o, op::RATE1 + 2)),
                    p.read(op_at(o, op::RATE1 + 3)),
                ],
                break_note: p.read(op_at(o, op::BREAK)),
                left: (
                    p.read(op_at(o, op::LEFT_CURVE)) as u32,
                    p.read(op_at(o, op::LEFT_DEPTH)),
                ),
                right: (
                    p.read(op_at(o, op::RIGHT_CURVE)) as u32,
                    p.read(op_at(o, op::RIGHT_DEPTH)),
                ),
                rate_scale: p.read(op_at(o, op::RATE_SCALE)),
                velocity: p.read(op_at(o, op::VELOCITY)),
                amp_mod: p.read(op_at(o, op::AMP_MOD)),
            }
        });
        let ams_units = |sens: f32| match sens as u32 {
            0 => 0.0,
            1 => 12.0,
            2 => 24.0,
            _ => 48.0,
        };
        let rate = self.rate;

        for vi in 0..self.voices.len() {
            let v = &mut self.voices[vi];
            if !v.on {
                continue;
            }
            // Once a chunk: the pitch with its glide and envelope, each
            // operator's frequency, its level offset from scaling,
            // velocity and the LFO, and its envelope's steps with the
            // rate scaling.
            v.pitch = glide.step(v.pitch, v.target, n, rate);
            let pitch_env_st = pitch_level_semitones(v.pitch_env.level);
            let pitch = v.pitch + transpose + bend + lfo_pitch + pitch_env_st;
            let base_hz = hz(pitch);
            let note = v.note as f32;
            let vel = v.velocity as f32 / 127.0;
            for (o, s) in v.ops.iter_mut().enumerate() {
                let now = &ops[o];
                let f = if now.fixed {
                    now.fixed_hz
                } else {
                    base_hz * now.ratio
                };
                s.dt = (f / rate).min(0.5);
                // Level scaling either side of the break point.
                let d = note - now.break_note;
                let (curve, depth) = if d < 0.0 { now.left } else { now.right };
                let d = d.abs();
                let shape = match curve {
                    0 | 3 => d / 48.0,
                    _ => ((d / 12.0).exp2() - 1.0) / 15.0,
                };
                let scaled = depth * shape * if curve < 2 { -1.0 } else { 1.0 };
                let velocity = now.velocity * 5.5 * vel.max(0.01).log2();
                let tremolo = -ams_units(now.amp_mod) * amd;
                s.gain = level_to_amp(now.level + scaled + velocity + tremolo);
                let scale = now.rate_scale * (note - 33.0) / 12.0;
                let rates = now.rates.map(|r| (r + scale).clamp(0.0, 99.0));
                let levels: [f32; 4] = std::array::from_fn(|i| p.read(op_at(o, op::LEVEL1 + i)));
                s.steps = EnvSteps::new(levels, rates, rate);
            }
            v.pitch_step = pitch_step;

            let mut done = false;
            for i in 0..count {
                v.pitch_env.next(&v.pitch_levels, &v.pitch_step);
                // The operators from the sixth down, so each hears its
                // modulators' outputs for this sample; feedback uses
                // the last two of its source's.
                let mut outs = [0.0f32; OPS];
                let mut sum = 0.0;
                for o in (0..OPS).rev() {
                    let s = &mut v.ops[o];
                    let amp = s.env.next(&s.steps) * s.gain;
                    let mods = algorithm.modulators[o];
                    let mut m = 0.0;
                    for (j, y) in outs.iter().enumerate() {
                        if mods & (1 << j) != 0 {
                            m += y;
                        }
                    }
                    if algorithm.feedback_to as usize == o + 1 {
                        let src = &v.ops[algorithm.feedback_from as usize - 1];
                        m += fb_gain / INDEX * 0.5 * (src.out + src.prev);
                    }
                    let s = &mut v.ops[o];
                    let y = amp * self.sine.at(s.phase + m * INDEX / std::f32::consts::TAU);
                    s.phase += s.dt;
                    if s.phase >= 1.0 {
                        s.phase -= 1.0;
                    }
                    s.prev = s.out;
                    s.out = y;
                    outs[o] = y;
                    if algorithm.carriers & (1 << o) != 0 {
                        sum += y;
                    }
                }
                // Released and every carrier idle: a short fade, then
                // the voice is free.
                let carriers_idle =
                    (0..OPS).all(|o| algorithm.carriers & (1 << o) == 0 || v.ops[o].env.idle());
                if carriers_idle {
                    v.fading -= 1.0 / (0.02 * rate);
                    if v.fading <= 0.0 {
                        done = true;
                    }
                }
                let out = sum * gain * v.fading.max(0.0);
                left[from + i] += out;
                right[from + i] += out;
                if done {
                    break;
                }
            }
            if done {
                v.on = false;
                self.alloc.free(vi);
            }
        }

        // The voices' sum, limited softly as the output stage does.
        for i in from..to {
            left[i] = soft(left[i] * 1.5) / 1.5;
            right[i] = soft(right[i] * 1.5) / 1.5;
        }
    }
}

impl Instrument for LuckySeven {
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

    fn synth() -> LuckySeven {
        LuckySeven::new(RATE as u32, Arc::new(Params::new(params())))
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    /// The level at `hz` in `x`, in dB.
    pub(super) fn db_at(x: &[f32], hz: f32) -> f32 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, &v) in x.iter().enumerate() {
            let p = std::f64::consts::TAU * hz as f64 * i as f64 / RATE as f64;
            re += v as f64 * p.cos();
            im += v as f64 * p.sin();
        }
        let a = (re * re + im * im).sqrt() * 2.0 / x.len() as f64;
        20.0 * a.max(1e-9).log10() as f32
    }

    fn render(s: &mut LuckySeven, events: &[Event], n: usize) -> Vec<f32> {
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        s.process(events, &mut l, &mut r);
        l
    }

    fn sounding(s: &LuckySeven) -> usize {
        s.voices.iter().filter(|v| v.on).count()
    }

    fn set(s: &LuckySeven, key: &str, v: f64) {
        s.params.shared().set_key(key, v);
    }

    #[test]
    fn the_keys_and_groups_cover_every_setting_once() {
        let p = params();
        assert_eq!(p.len(), at::COUNT);
        let mut keys: Vec<&str> = p.iter().map(|p| p.key.as_str()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), p.len());
        assert_eq!(p[op_at(2, op::LEVEL1 + 3)].key, "op3_level4");
        assert_eq!(p[op_at(5, op::RATE1)].key, "op6_rate1");
        assert_eq!(p[at::ALGORITHM].key, "algorithm");
        assert_eq!(p[at::LEVEL].key, "level");
        let mut covered = vec![false; p.len()];
        for &(_, from, to) in GROUPS {
            for c in &mut covered[from..=to] {
                assert!(!*c);
                *c = true;
            }
        }
        assert!(covered.iter().all(|c| *c));
    }

    #[test]
    fn every_algorithm_hears_something_and_modulators_point_down() {
        for (i, a) in ALGORITHM_TABLE.iter().enumerate() {
            assert!(a.carriers != 0, "algorithm {}", i + 1);
            assert!((1..=6).contains(&a.feedback_from) && (1..=6).contains(&a.feedback_to));
            for (o, m) in a.modulators.iter().enumerate() {
                // Every modulator has a higher number than what it
                // modulates, so one pass from the sixth down works.
                for j in 0..OPS {
                    if m & (1 << j) != 0 {
                        assert!(j > o, "algorithm {}: op {} ← op {}", i + 1, o + 1, j + 1);
                    }
                }
                // A carrier isn't also a modulator's input, and vice
                // versa, except through feedback.
                if a.carriers & (1 << o) == 0 {
                    assert!(
                        a.modulators.iter().any(|m| m & (1 << o) != 0),
                        "algorithm {}: op {} goes nowhere",
                        i + 1,
                        o + 1
                    );
                }
            }
        }
        assert_eq!(algorithm(32.0).carriers, 0b11_1111);
        assert_eq!(algorithm(0.0), &ALGORITHM_TABLE[0]);
    }

    #[test]
    fn the_level_scale_is_three_quarters_of_a_decibel_a_step_near_the_top() {
        let dbs = |u: f32| 20.0 * level_to_amp(u).log10();
        assert!((dbs(99.0)).abs() < 1e-3);
        assert!((dbs(91.0) + 6.0).abs() < 0.05, "{}", dbs(91.0));
        assert!((dbs(20.0) + 59.5).abs() < 0.1, "{}", dbs(20.0));
        assert!(dbs(10.0) < -70.0);
        assert_eq!(level_to_amp(0.0), 0.0);
        for u in [5.0, 19.0, 20.0, 21.0, 50.0, 99.0] {
            assert!((amp_to_level(level_to_amp(u)) - u).abs() < 1e-2, "{u}");
        }
        // Rates: 99 is near-instant, 50 about half a second for the
        // whole range, 0 over a minute.
        assert!(rate_seconds(99.0) < 0.005);
        assert!(
            (rate_seconds(50.0) - 0.5).abs() < 0.1,
            "{}",
            rate_seconds(50.0)
        );
        assert!(rate_seconds(0.0) > 60.0);
    }

    #[test]
    fn a_single_sine_operator_plays_the_note_and_the_ratio_moves_it() {
        let mut s = synth();
        set(&s, "op2_level", 0.0);
        let x = render(&mut s, &[Event::note_on(0, 0, 69, 100, 0)], 48_000);
        let tail = &x[24_000..];
        assert!(db_at(tail, 440.0) > -20.0, "{}", db_at(tail, 440.0));
        assert!(db_at(tail, 880.0) < db_at(tail, 440.0) - 40.0);
        // Coarse 2 is an octave up; fine 50 is half as much again;
        // coarse 0 is an octave down.
        for (coarse, fine, hz) in [(2.0, 0.0, 880.0), (2.0, 50.0, 1320.0), (0.0, 0.0, 220.0)] {
            let mut s = synth();
            set(&s, "op2_level", 0.0);
            set(&s, "op1_coarse", coarse);
            set(&s, "op1_fine", fine);
            let x = render(&mut s, &[Event::note_on(0, 0, 69, 100, 0)], 48_000);
            assert!(
                db_at(&x[24_000..], hz) > -20.0,
                "{coarse} {fine}: {}",
                db_at(&x[24_000..], hz)
            );
        }
        // Fixed: coarse 2 is 100 Hz whatever the note; fine 30 is
        // 10^0.3 of that, about 200 Hz.
        let mut s = synth();
        set(&s, "op2_level", 0.0);
        set(&s, "op1_mode", 1.0);
        set(&s, "op1_coarse", 2.0);
        let x = render(&mut s, &[Event::note_on(0, 0, 69, 100, 0)], 48_000);
        assert!(db_at(&x[24_000..], 100.0) > -20.0);
        set(&s, "op1_fine", 30.0);
        let x = render(&mut s, &[], 48_000);
        assert!(
            db_at(&x[24_000..], 199.5) > -20.0,
            "{}",
            db_at(&x[24_000..], 199.5)
        );
    }

    #[test]
    fn a_modulator_adds_harmonics_in_proportion_to_its_level() {
        let harmonics = |level: f64| {
            let mut s = synth();
            set(&s, "op2_level", level);
            let x = render(&mut s, &[Event::note_on(0, 0, 57, 100, 0)], 48_000);
            let f0 = hz(57.0);
            (db_at(&x[24_000..], f0), db_at(&x[24_000..], f0 * 3.0))
        };
        let (f_plain, h_plain) = harmonics(0.0);
        let (_, h_mild) = harmonics(70.0);
        let (_, h_wild) = harmonics(99.0);
        assert!(h_plain < f_plain - 40.0);
        assert!(h_mild > h_plain + 20.0, "{h_plain} {h_mild}");
        assert!(h_wild > h_mild + 6.0, "{h_mild} {h_wild}");
        // Feedback on a lone carrier gives it harmonics too.
        // Feedback on a lone carrier (the sixth, in algorithm 32)
        // gives it harmonics too, more with more feedback.
        let second = |fb: f64| {
            let mut s = synth();
            set(&s, "op1_level", 0.0);
            set(&s, "op6_level", 99.0);
            set(&s, "algorithm", 32.0);
            set(&s, "feedback", fb);
            let x = render(&mut s, &[Event::note_on(0, 0, 57, 100, 0)], 48_000);
            let f0 = hz(57.0);
            assert!(peak(&x) < 1.0);
            db_at(&x[24_000..], 2.0 * f0) - db_at(&x[24_000..], f0)
        };
        assert!(second(0.0) < -40.0, "{}", second(0.0));
        assert!(second(4.0) > second(0.0) + 20.0, "{}", second(4.0));
        assert!(
            second(7.0) > second(4.0) + 3.0,
            "{} {}",
            second(4.0),
            second(7.0)
        );
        assert!(second(7.0) > -12.0, "{}", second(7.0));
    }

    #[test]
    fn the_envelope_rises_falls_holds_and_releases_on_its_rates() {
        let mut s = synth();
        set(&s, "op2_level", 0.0);
        set(&s, "op1_rate1", 50.0);
        set(&s, "op1_level2", 50.0);
        set(&s, "op1_rate2", 60.0);
        set(&s, "op1_level3", 50.0);
        set(&s, "op1_rate4", 70.0);
        let x = render(&mut s, &[Event::note_on(0, 0, 60, 127, 0)], 96_000);
        // Half a second's rise at rate 50: louder at 0.4 s than at 0.1.
        let at = |t: f32| peak(&x[(t * RATE) as usize..(t * RATE) as usize + 2000]);
        assert!(at(0.1) < at(0.4), "{} {}", at(0.1), at(0.4));
        assert!(at(0.4) > 0.15, "{}", at(0.4));
        // Then down to level 50 (about -37 dB) and held there.
        assert!(at(1.5) < 0.05, "{}", at(1.5));
        assert!((at(1.5) - at(1.9)).abs() < 0.005);
        assert_eq!(sounding(&s), 1);
        let x = render(&mut s, &[Event::note_off(0, 0, 60, 0)], 48_000);
        assert!(peak(&x[40_000..]) < 1e-4);
        assert_eq!(sounding(&s), 0);
    }

    #[test]
    fn rate_scaling_shortens_the_envelope_up_the_keyboard() {
        let decay = |note: u8, scale: f64| {
            let mut s = synth();
            set(&s, "op2_level", 0.0);
            set(&s, "op1_rate2", 40.0);
            set(&s, "op1_level2", 0.0);
            set(&s, "op1_level3", 0.0);
            set(&s, "op1_rate_scale", scale);
            let x = render(&mut s, &[Event::note_on(0, 0, note, 127, 0)], 48_000);
            peak(&x[24_000..30_000])
        };
        assert!((decay(36, 0.0) - decay(96, 0.0)).abs() < 0.05);
        assert!(
            decay(96, 7.0) < decay(36, 7.0) * 0.3,
            "{} {}",
            decay(36, 7.0),
            decay(96, 7.0)
        );
    }

    #[test]
    fn level_scaling_and_velocity_shape_the_level_as_set() {
        let level = |note: u8, vel: u8, curve: f64, depth: f64| {
            let mut s = synth();
            set(&s, "op2_level", 0.0);
            set(&s, "op1_break", 60.0);
            set(&s, "op1_right_curve", curve);
            set(&s, "op1_right_depth", depth);
            set(&s, "op1_velocity", 7.0);
            let x = render(&mut s, &[Event::note_on(0, 0, note, vel, 0)], 12_000);
            peak(&x[6000..])
        };
        let plain = level(84, 127, 0.0, 0.0);
        assert!(level(84, 127, 0.0, 99.0) < plain * 0.3, "-linear cuts");
        assert!(level(84, 127, 1.0, 99.0) < plain * 0.3, "-exponential cuts");
        assert!(
            level(84, 127, 1.0, 99.0) > level(84, 127, 0.0, 99.0),
            "less steeply"
        );
        assert!(
            level(60, 127, 0.0, 99.0) > plain * 0.9,
            "nothing at the break point"
        );
        assert!(
            level(84, 64, 0.0, 0.0) < plain * 0.4,
            "velocity 64 is well down at 7"
        );
        let mut s = synth();
        set(&s, "op2_level", 0.0);
        let x = render(&mut s, &[Event::note_on(0, 0, 84, 64, 0)], 12_000);
        assert!(
            (peak(&x[6000..]) - plain).abs() < 0.02,
            "velocity 0 ignores it"
        );
    }

    #[test]
    fn the_lfo_wobbles_the_pitch_and_the_pitch_envelope_bends_it() {
        let mut s = synth();
        set(&s, "op2_level", 0.0);
        set(&s, "lfo_pitch", 99.0);
        set(&s, "pitch_mod_sens", 7.0);
        set(&s, "lfo_speed", 0.0);
        set(&s, "lfo_wave", 3.0);
        set(&s, "lfo_sync", 1.0);
        render(&mut s, &[Event::note_on(0, 0, 69, 100, 0)], 2000);
        // A square at its top: an octave up.
        let f = s.voices[0].ops[0].dt * RATE;
        assert!((f - 880.0).abs() < 5.0, "{f}");
        // The wheel brings the LFO in when the depth is zero.
        let mut s = synth();
        set(&s, "lfo_pitch", 0.0);
        set(&s, "pitch_mod_sens", 7.0);
        set(&s, "lfo_speed", 0.0);
        set(&s, "lfo_wave", 3.0);
        set(&s, "wheel", 99.0);
        let wheel = Event {
            frame: 0,
            channel: 0,
            kind: Kind::Control {
                number: 1,
                value: 127,
            },
        };
        render(&mut s, &[wheel, Event::note_on(0, 0, 69, 100, 0)], 2000);
        let f = s.voices[0].ops[0].dt * RATE;
        assert!((f - 880.0).abs() < 5.0, "{f}");
        // The pitch envelope: from a fifth up at its fourth level, down
        // to the note at rate 50.
        let mut s = synth();
        set(&s, "pitch_level4", 75.0);
        set(&s, "pitch_level1", 50.0);
        set(&s, "pitch_rate1", 50.0);
        render(&mut s, &[Event::note_on(0, 0, 69, 100, 0)], 64);
        let f = s.voices[0].ops[0].dt * RATE;
        assert!((f - 880.0).abs() < 10.0, "{f}");
        render(&mut s, &[], 96_000);
        let f = s.voices[0].ops[0].dt * RATE;
        assert!((f - 440.0).abs() < 1.0, "{f}");
    }

    #[test]
    fn sixteen_voices_then_mono_with_glide_and_legato() {
        let mut s = synth();
        set(&s, "op1_level3", 99.0);
        let events: Vec<Event> = (0..17)
            .map(|i| Event::note_on(i * 10, 0, 48 + i as u8, 100, i))
            .collect();
        render(&mut s, &events, 256);
        assert_eq!(sounding(&s), 16);
        assert_eq!(
            s.alloc.note_of(0),
            Some(64),
            "the first note's voice was taken"
        );
        let mut s = synth();
        set(&s, "playing", 1.0);
        set(&s, "glide", 1.0);
        set(&s, "op1_level3", 99.0);
        render(
            &mut s,
            &[
                Event::note_on(0, 0, 48, 100, 0),
                Event::note_on(1000, 0, 60, 100, 1),
            ],
            1100,
        );
        assert_eq!(sounding(&s), 1);
        assert!(s.voices[0].pitch < 50.0, "{}", s.voices[0].pitch);
        // Legato: the second note doesn't restart the envelope.
        let mut s = synth();
        set(&s, "playing", 2.0);
        set(&s, "glide", 0.005);
        set(&s, "op1_rate1", 30.0);
        set(&s, "op1_level3", 99.0);
        let x = render(
            &mut s,
            &[
                Event::note_on(0, 0, 48, 100, 0),
                Event::note_on(24_000, 0, 55, 100, 1),
            ],
            48_000,
        );
        assert!(peak(&x[23_000..24_000]) < peak(&x[25_000..26_000]) * 1.2);
        assert!(s.voices[0].ops[0].env.level() > 60.0);
    }

    #[test]
    fn the_sync_switch_restarts_the_operators_and_off_leaves_them_running() {
        let mut s = synth();
        set(&s, "op2_level", 0.0);
        render(&mut s, &[Event::note_on(0, 0, 60, 100, 0)], 1000);
        render(
            &mut s,
            &[
                Event::note_off(0, 0, 60, 0),
                Event::note_on(1, 0, 60, 100, 1),
            ],
            2,
        );
        assert!(
            s.voices[0].ops[0].phase < 0.01,
            "{}",
            s.voices[0].ops[0].phase
        );
        let mut s = synth();
        set(&s, "osc_sync", 0.0);
        render(&mut s, &[Event::note_on(0, 0, 60, 100, 0)], 1000);
        let before = s.voices[0].ops[0].phase;
        render(
            &mut s,
            &[
                Event::note_off(0, 0, 60, 0),
                Event::note_on(1, 0, 60, 100, 1),
            ],
            2,
        );
        let after = s.voices[0].ops[0].phase;
        assert!((after - before).abs() > 0.001 || before > 0.01);
        assert!(after > 0.01, "{after}");
    }

    /// The golden render: a fixed sound has a known spectrum, so a
    /// later change can't silently change it.
    #[test]
    fn the_golden_tine_renders_the_same_spectrum() {
        let b = crate::Builtin::new(crate::Kind::LuckySeven);
        b.set_sound("Tine piano").unwrap();
        b.params().set_key("lfo_amp", 0.0);
        let mut s = LuckySeven::new(RATE as u32, b.params().clone());
        let x = render(&mut s, &[Event::note_on(0, 0, 60, 110, 0)], 48_000);
        let f0 = hz(60.0);
        let got: Vec<f32> = [0.5, 1.0, 2.0, 3.0, 15.0]
            .iter()
            .map(|h| db_at(&x[..24_000], f0 * h))
            .collect();
        for (g, w) in got.iter().zip(GOLDEN) {
            assert!((g - w).abs() < 1.5, "got {got:.1?}, want {GOLDEN:?}");
        }
    }

    /// Levels in dB at half, one, two, three and fourteen times the
    /// fundamental of the golden tine piano at C4 (the tine sits at
    /// fifteen times, the sum of the carrier and its ratio-14 modulator).
    const GOLDEN: [f32; 5] = [-62.0, -13.3, -21.6, -29.1, -50.3];

    /// The plan's CPU budget: sixteen voices with every operator on,
    /// a few percent of one core at 48 kHz.
    #[test]
    #[cfg_attr(debug_assertions, ignore = "timed in release builds only")]
    fn sixteen_voices_fit_the_budget() {
        let b = crate::Builtin::new(crate::Kind::LuckySeven);
        b.set_sound("Brass section").unwrap();
        let mut s = LuckySeven::new(RATE as u32, b.params().clone());
        let seconds = 2.0;
        let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
        let chord: Vec<Event> = (0..16)
            .map(|i| Event::note_on(0, 0, 40 + 2 * i as u8, 100, i))
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
