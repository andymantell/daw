// SPDX-License-Identifier: GPL-3.0-or-later
//! Eighty: an eight-voice two-channel touch polysynth built the way the
//! 1977 Japanese classic was (`docs/plans/classic-synths.md`, S6), for
//! big pads, brass and expressive leads. Every voice has two complete
//! channels, each with a saw and a square (with a pulse width that its
//! own slow sweep can move) and noise, a resonant high-pass into a
//! resonant low-pass (12 or 24 dB), a sine added after the filters, a
//! filter envelope that goes from an initial level up to an attack level
//! and back down while the key is held, and a loudness envelope; the
//! two channels are mixed. How hard a key is struck and how hard it is
//! held down afterwards (polyphonic aftertouch, from a keyboard or from
//! a note's pressure line in the editor) set the level, the brightness
//! and the vibrato. A sub-oscillator (the LFO) gives vibrato, a filter
//! wobble and tremolo; a ring modulator with its own envelope works on
//! the sum of the voices; the bucket-brigade chorus widens it; the pitch
//! wheel plays the ribbon.
//!
//! Everything nonlinear is where the circuit has it: the filters'
//! saturating loops, the amplifier's soft limit. The oscillators drift a
//! little, each voice and channel on its own, by the vintage amount.
//! The filters run at the sample rate rather than twice it (two-pole
//! filters with mild saturation fold back little, and there are four a
//! voice), so eight voices stay affordable.

use std::f32::consts::TAU;

use daw_engine::midi::{Event, Expression, Instrument, Kind};

use crate::dsp::{Noise, db, hz};
use crate::params::{Live, Param, Unit};
use crate::vintage::{
    Action, Allocator, Analog, Chorus, ChorusMode, Drift, Glide, GlideMode, Osc, Priority, Shape,
    Steps, Svf, SvfOut, VoiceSettings, Wave, soft,
};

const SLOPES: &[&str] = &["12 dB", "24 dB"];
const LFO_SHAPES: &[&str] = &["Sine", "Saw up", "Saw down", "Square", "Random"];

/// The most voices.
pub const MAX_VOICES: usize = 16;

/// A channel's settings, by index within its block; channel 1's block
/// starts at [`at::CH1`], channel 2's at [`at::CH2`].
pub mod ch {
    pub const SAW: usize = 0;
    pub const SQUARE: usize = 1;
    pub const WIDTH: usize = 2;
    pub const PWM: usize = 3;
    pub const PWM_SPEED: usize = 4;
    pub const NOISE: usize = 5;
    pub const SINE: usize = 6;
    pub const TUNE: usize = 7;
    pub const HP: usize = 8;
    pub const HP_RES: usize = 9;
    pub const LP: usize = 10;
    pub const LP_RES: usize = 11;
    pub const SLOPE: usize = 12;
    pub const F_INITIAL: usize = 13;
    pub const F_ATTACK_LEVEL: usize = 14;
    pub const F_ATTACK: usize = 15;
    pub const F_DECAY: usize = 16;
    pub const F_RELEASE: usize = 17;
    pub const ATTACK: usize = 18;
    pub const DECAY: usize = 19;
    pub const SUSTAIN: usize = 20;
    pub const RELEASE: usize = 21;
    pub const LEVEL: usize = 22;
    /// Settings in a channel's block.
    pub const LEN: usize = 23;
}

/// Settings by index; the order of [`params`].
pub mod at {
    use super::ch;
    pub const CH1: usize = 0;
    pub const CH2: usize = ch::LEN;
    pub const MIX: usize = 2 * ch::LEN;
    pub const BRILLIANCE: usize = MIX + 1;
    pub const KEY_TRACK: usize = MIX + 2;
    pub const TOUCH_LEVEL: usize = MIX + 3;
    pub const TOUCH_CUTOFF: usize = MIX + 4;
    pub const AFTER_LEVEL: usize = MIX + 5;
    pub const AFTER_CUTOFF: usize = MIX + 6;
    pub const AFTER_VIBRATO: usize = MIX + 7;
    pub const LFO_SPEED: usize = MIX + 8;
    pub const LFO_SHAPE: usize = MIX + 9;
    pub const LFO_PITCH: usize = MIX + 10;
    pub const LFO_FILTER: usize = MIX + 11;
    pub const LFO_LEVEL: usize = MIX + 12;
    pub const RING_DEPTH: usize = MIX + 13;
    pub const RING_SPEED: usize = MIX + 14;
    pub const RING_SWEEP: usize = MIX + 15;
    pub const RING_ATTACK: usize = MIX + 16;
    pub const RING_DECAY: usize = MIX + 17;
    pub const CHORUS: usize = MIX + 18;
    pub const VOICES: usize = MIX + 19;
    pub const GLIDE: usize = MIX + 20;
    pub const BEND: usize = MIX + 21;
    pub const LEVEL: usize = MIX + 22;
    pub const VINTAGE: usize = MIX + 23;
    pub const LEN: usize = MIX + 24;
}

/// The index of channel `c` (0 or 1)'s setting `k`.
pub fn ch_at(c: usize, k: usize) -> usize {
    c * ch::LEN + k
}

/// The key of channel `c` (0 or 1)'s setting `k`, as projects name it.
pub fn ch_key(c: usize, k: &str) -> String {
    format!("ch{}_{k}", c + 1)
}

/// One channel's settings, in the order of [`ch`].
fn channel_params(c: usize) -> Vec<Param> {
    use Unit::*;
    let key = |k: &str| ch_key(c, k);
    let name = |n: &str| format!("Ch {} {n}", c + 1);
    let (saw, square, tune) = if c == 0 {
        (1.0, 0.0, 0.0)
    } else {
        (0.0, 1.0, 6.0)
    };
    vec![
        Param::new(key("saw"), name("saw"), 0.0, 1.0, saw, Switch),
        Param::new(key("square"), name("square"), 0.0, 1.0, square, Switch),
        Param::new(key("width"), name("pulse width"), 0.05, 0.95, 0.5, Amount),
        Param::new(key("pwm"), name("pulse width sweep"), 0.0, 1.0, 0.0, Amount),
        Param::new(key("pwm_speed"), name("sweep speed"), 0.1, 20.0, 0.6, Hz).log(),
        Param::new(key("noise"), name("noise"), 0.0, 1.0, 0.0, Amount),
        Param::new(key("sine"), name("sine"), 0.0, 1.0, 0.0, Amount),
        Param::new(key("tune"), name("tune"), -100.0, 100.0, tune, Cents),
        Param::new(key("hp"), name("high-pass"), 20.0, 18_000.0, 20.0, Hz).log(),
        Param::new(
            key("hp_res"),
            name("high-pass resonance"),
            0.0,
            1.0,
            0.0,
            Amount,
        ),
        Param::new(key("lp"), name("low-pass"), 20.0, 18_000.0, 1400.0, Hz).log(),
        Param::new(
            key("lp_res"),
            name("low-pass resonance"),
            0.0,
            1.0,
            0.15,
            Amount,
        ),
        Param::new(key("slope"), name("filter slope"), 0.0, 1.0, 0.0, Choice).choices(SLOPES),
        Param::new(
            key("f_initial"),
            name("filter initial level"),
            -4.0,
            4.0,
            0.0,
            Octaves,
        ),
        Param::new(
            key("f_attack_level"),
            name("filter attack level"),
            -4.0,
            4.0,
            1.5,
            Octaves,
        ),
        Param::new(
            key("f_attack"),
            name("filter attack"),
            0.001,
            10.0,
            0.01,
            Seconds,
        )
        .log(),
        Param::new(
            key("f_decay"),
            name("filter decay"),
            0.004,
            10.0,
            0.6,
            Seconds,
        )
        .log(),
        Param::new(
            key("f_release"),
            name("filter release"),
            0.004,
            10.0,
            0.3,
            Seconds,
        )
        .log(),
        Param::new(key("attack"), name("attack"), 0.001, 10.0, 0.01, Seconds).log(),
        Param::new(key("decay"), name("decay"), 0.004, 10.0, 0.5, Seconds).log(),
        Param::new(key("sustain"), name("sustain"), 0.0, 1.0, 0.7, Amount),
        Param::new(key("release"), name("release"), 0.004, 10.0, 0.4, Seconds).log(),
        Param::new(key("level"), name("level"), 0.0, 1.0, 0.8, Amount),
    ]
}

/// The synth's settings, in the order of [`at`]. Grouped for showing by
/// [`GROUPS`].
pub fn params() -> Vec<Param> {
    use Unit::*;
    let mut v = channel_params(0);
    v.extend(channel_params(1));
    v.extend([
        Param::new("mix", "Channel mix (1 to 2)", 0.0, 1.0, 0.5, Amount),
        Param::new("brilliance", "Brilliance", -3.0, 3.0, 0.0, Octaves),
        Param::new("key_track", "Keyboard tracking", 0.0, 1.0, 1.0, Amount),
        Param::new("touch_level", "Velocity to level", 0.0, 1.0, 0.5, Amount),
        Param::new(
            "touch_cutoff",
            "Velocity to brightness",
            0.0,
            4.0,
            1.0,
            Octaves,
        ),
        Param::new("after_level", "Aftertouch to level", 0.0, 1.0, 0.4, Amount),
        Param::new(
            "after_cutoff",
            "Aftertouch to brightness",
            0.0,
            4.0,
            1.0,
            Octaves,
        ),
        Param::new(
            "after_vibrato",
            "Aftertouch to vibrato",
            0.0,
            1.0,
            0.3,
            Semitones,
        ),
        Param::new("lfo_speed", "Sub-oscillator speed", 0.1, 30.0, 5.5, Hz).log(),
        Param::new("lfo_shape", "Sub-oscillator shape", 0.0, 4.0, 0.0, Choice).choices(LFO_SHAPES),
        Param::new("lfo_pitch", "Vibrato", 0.0, 2.0, 0.0, Semitones),
        Param::new(
            "lfo_filter",
            "Sub-oscillator to filter",
            0.0,
            4.0,
            0.0,
            Octaves,
        ),
        Param::new("lfo_level", "Tremolo", 0.0, 1.0, 0.0, Amount),
        Param::new("ring_depth", "Ring modulator depth", 0.0, 1.0, 0.0, Amount),
        Param::new("ring_speed", "Ring modulator speed", 0.5, 2000.0, 120.0, Hz).log(),
        Param::new(
            "ring_sweep",
            "Ring envelope to speed",
            -6.0,
            6.0,
            0.0,
            Octaves,
        ),
        Param::new(
            "ring_attack",
            "Ring envelope attack",
            0.001,
            10.0,
            0.1,
            Seconds,
        )
        .log(),
        Param::new(
            "ring_decay",
            "Ring envelope decay",
            0.004,
            10.0,
            1.0,
            Seconds,
        )
        .log(),
        Param::new("chorus", "Chorus", 0.0, 3.0, 0.0, Choice).choices(ChorusMode::NAMES),
        Param::new("voices", "Voices", 1.0, MAX_VOICES as f64, 8.0, Count),
        Param::new("glide", "Glide time", 0.0, 2.0, 0.0, Seconds),
        Param::new("bend", "Ribbon (bend) range", 0.0, 24.0, 2.0, Semitones),
        Param::new("level", "Level", -36.0, 6.0, -6.0, Db),
        Param::new("vintage", "Vintage drift", 0.0, 1.0, 0.4, Amount),
    ]);
    debug_assert_eq!(v.len(), at::LEN);
    v
}

/// The settings in groups, for the controls: (heading, first, last).
pub const GROUPS: &[(&str, usize, usize)] = &[
    ("Channel 1", at::CH1, at::CH1 + ch::LEN - 1),
    ("Channel 2", at::CH2, at::CH2 + ch::LEN - 1),
    ("Mix and touch", at::MIX, at::AFTER_VIBRATO),
    ("Sub-oscillator", at::LFO_SPEED, at::LFO_LEVEL),
    ("Ring modulator", at::RING_DEPTH, at::RING_DECAY),
    ("Playing", at::CHORUS, at::VINTAGE),
];

/// Sounds made here, as settings over the defaults: pads, brass, leads
/// and keys, for the content browser and the assistant.
pub fn sounds() -> Vec<crate::Sound> {
    use crate::Sound;
    vec![
        Sound {
            name: "Big pad",
            settings: &[
                ("ch1_saw", 1.0),
                ("ch1_square", 0.0),
                ("ch1_lp", 900.0),
                ("ch1_lp_res", 0.2),
                ("ch1_f_attack_level", 1.2),
                ("ch1_f_attack", 0.9),
                ("ch1_f_decay", 2.0),
                ("ch1_f_release", 1.5),
                ("ch1_attack", 0.7),
                ("ch1_decay", 1.0),
                ("ch1_sustain", 0.9),
                ("ch1_release", 1.8),
                ("ch2_saw", 0.0),
                ("ch2_square", 1.0),
                ("ch2_width", 0.5),
                ("ch2_pwm", 0.35),
                ("ch2_pwm_speed", 0.4),
                ("ch2_tune", -7.0),
                ("ch2_lp", 1100.0),
                ("ch2_lp_res", 0.15),
                ("ch2_f_attack_level", 1.0),
                ("ch2_f_attack", 1.2),
                ("ch2_f_decay", 2.5),
                ("ch2_f_release", 1.5),
                ("ch2_attack", 0.9),
                ("ch2_decay", 1.0),
                ("ch2_sustain", 0.9),
                ("ch2_release", 2.0),
                ("after_cutoff", 1.5),
                ("lfo_speed", 4.8),
                ("lfo_pitch", 0.06),
                ("chorus", 3.0),
            ],
        },
        Sound {
            name: "Brass",
            settings: &[
                ("ch1_saw", 1.0),
                ("ch1_square", 0.0),
                ("ch1_lp", 700.0),
                ("ch1_lp_res", 0.25),
                ("ch1_f_initial", -0.5),
                ("ch1_f_attack_level", 2.5),
                ("ch1_f_attack", 0.08),
                ("ch1_f_decay", 0.35),
                ("ch1_f_release", 0.2),
                ("ch1_attack", 0.04),
                ("ch1_decay", 0.3),
                ("ch1_sustain", 0.85),
                ("ch1_release", 0.25),
                ("ch2_saw", 1.0),
                ("ch2_square", 0.0),
                ("ch2_tune", 9.0),
                ("ch2_lp", 800.0),
                ("ch2_lp_res", 0.2),
                ("ch2_f_initial", -0.5),
                ("ch2_f_attack_level", 2.2),
                ("ch2_f_attack", 0.12),
                ("ch2_f_decay", 0.4),
                ("ch2_f_release", 0.2),
                ("ch2_attack", 0.06),
                ("ch2_decay", 0.3),
                ("ch2_sustain", 0.85),
                ("ch2_release", 0.25),
                ("touch_cutoff", 1.5),
                ("after_cutoff", 1.2),
                ("after_level", 0.5),
                ("lfo_speed", 5.0),
                ("after_vibrato", 0.25),
            ],
        },
        Sound {
            name: "Strings",
            settings: &[
                ("ch1_saw", 1.0),
                ("ch1_square", 0.0),
                ("ch1_hp", 180.0),
                ("ch1_lp", 2200.0),
                ("ch1_lp_res", 0.1),
                ("ch1_f_attack_level", 0.5),
                ("ch1_f_attack", 0.5),
                ("ch1_f_decay", 1.0),
                ("ch1_attack", 0.35),
                ("ch1_decay", 0.5),
                ("ch1_sustain", 1.0),
                ("ch1_release", 0.9),
                ("ch2_saw", 1.0),
                ("ch2_square", 0.0),
                ("ch2_tune", -9.0),
                ("ch2_hp", 180.0),
                ("ch2_lp", 2600.0),
                ("ch2_lp_res", 0.1),
                ("ch2_f_attack_level", 0.5),
                ("ch2_f_attack", 0.6),
                ("ch2_f_decay", 1.0),
                ("ch2_attack", 0.45),
                ("ch2_decay", 0.5),
                ("ch2_sustain", 1.0),
                ("ch2_release", 1.0),
                ("lfo_speed", 5.3),
                ("lfo_pitch", 0.08),
                ("after_vibrato", 0.4),
                ("chorus", 3.0),
                ("vintage", 0.6),
            ],
        },
        Sound {
            name: "Expressive lead",
            settings: &[
                ("ch1_saw", 1.0),
                ("ch1_square", 1.0),
                ("ch1_width", 0.3),
                ("ch1_lp", 1600.0),
                ("ch1_lp_res", 0.35),
                ("ch1_f_attack_level", 1.5),
                ("ch1_f_attack", 0.02),
                ("ch1_f_decay", 0.5),
                ("ch1_f_release", 0.3),
                ("ch1_attack", 0.005),
                ("ch1_decay", 0.4),
                ("ch1_sustain", 0.8),
                ("ch1_release", 0.3),
                ("ch2_saw", 1.0),
                ("ch2_square", 0.0),
                ("ch2_tune", 5.0),
                ("ch2_sine", 0.4),
                ("ch2_lp", 1200.0),
                ("ch2_lp_res", 0.3),
                ("ch2_f_attack_level", 1.5),
                ("ch2_f_attack", 0.02),
                ("ch2_f_decay", 0.5),
                ("ch2_f_release", 0.3),
                ("ch2_attack", 0.005),
                ("ch2_decay", 0.4),
                ("ch2_sustain", 0.8),
                ("ch2_release", 0.3),
                ("mix", 0.4),
                ("touch_cutoff", 2.0),
                ("after_cutoff", 2.0),
                ("after_level", 0.6),
                ("after_vibrato", 0.5),
                ("lfo_speed", 6.0),
                ("voices", 1.0),
                ("glide", 0.08),
            ],
        },
        Sound {
            name: "Soft keys",
            settings: &[
                ("ch1_saw", 0.0),
                ("ch1_square", 0.0),
                ("ch1_sine", 0.9),
                ("ch1_lp", 3000.0),
                ("ch1_attack", 0.004),
                ("ch1_decay", 1.2),
                ("ch1_sustain", 0.2),
                ("ch1_release", 0.5),
                ("ch2_saw", 0.0),
                ("ch2_square", 1.0),
                ("ch2_width", 0.2),
                ("ch2_tune", 3.0),
                ("ch2_hp", 120.0),
                ("ch2_lp", 1500.0),
                ("ch2_lp_res", 0.2),
                ("ch2_f_attack_level", 2.0),
                ("ch2_f_attack", 0.003),
                ("ch2_f_decay", 0.25),
                ("ch2_f_release", 0.2),
                ("ch2_attack", 0.003),
                ("ch2_decay", 0.6),
                ("ch2_sustain", 0.15),
                ("ch2_release", 0.4),
                ("mix", 0.4),
                ("touch_level", 0.7),
                ("touch_cutoff", 2.5),
                ("after_level", 0.0),
            ],
        },
        Sound {
            name: "Pressure bass",
            settings: &[
                ("ch1_saw", 1.0),
                ("ch1_square", 1.0),
                ("ch1_width", 0.5),
                ("ch1_lp", 500.0),
                ("ch1_lp_res", 0.3),
                ("ch1_f_initial", -0.5),
                ("ch1_f_attack_level", 2.5),
                ("ch1_f_attack", 0.004),
                ("ch1_f_decay", 0.3),
                ("ch1_f_release", 0.2),
                ("ch1_attack", 0.003),
                ("ch1_decay", 0.4),
                ("ch1_sustain", 0.6),
                ("ch1_release", 0.2),
                ("ch2_saw", 0.0),
                ("ch2_square", 0.0),
                ("ch2_sine", 1.0),
                ("ch2_tune", 0.0),
                ("ch2_lp", 400.0),
                ("ch2_attack", 0.003),
                ("ch2_decay", 0.4),
                ("ch2_sustain", 0.8),
                ("ch2_release", 0.2),
                ("mix", 0.55),
                ("key_track", 0.5),
                ("touch_cutoff", 1.5),
                ("after_cutoff", 2.5),
                ("after_level", 0.3),
                ("after_vibrato", 0.0),
                ("voices", 2.0),
            ],
        },
        Sound {
            name: "Ring bells",
            settings: &[
                ("ch1_saw", 0.0),
                ("ch1_square", 1.0),
                ("ch1_width", 0.5),
                ("ch1_lp", 4000.0),
                ("ch1_lp_res", 0.3),
                ("ch1_f_attack_level", 0.0),
                ("ch1_attack", 0.002),
                ("ch1_decay", 2.5),
                ("ch1_sustain", 0.0),
                ("ch1_release", 2.5),
                ("ch2_saw", 0.0),
                ("ch2_square", 0.0),
                ("ch2_sine", 1.0),
                ("ch2_tune", 2.0),
                ("ch2_lp", 6000.0),
                ("ch2_attack", 0.002),
                ("ch2_decay", 3.0),
                ("ch2_sustain", 0.0),
                ("ch2_release", 3.0),
                ("ring_depth", 0.8),
                ("ring_speed", 310.0),
                ("ring_sweep", -1.5),
                ("ring_attack", 0.002),
                ("ring_decay", 1.5),
                ("touch_level", 0.7),
                ("chorus", 1.0),
            ],
        },
    ]
}

/// Settings are read, and the filters set, this often.
const CHUNK: usize = 16;
/// What each voice is scaled by before the amplifier's soft limit.
const HEADROOM: f32 = 0.3;

/// One channel's settings for a chunk, read once for every voice.
#[derive(Clone, Copy)]
struct ChanSet {
    saw: bool,
    square: bool,
    width: f32,
    noise: f32,
    sine: f32,
    /// Semitones from the note.
    tune: f32,
    hp: f32,
    hp_res: f32,
    hp_on: bool,
    lp: f32,
    lp_res: f32,
    four_pole: bool,
    f_initial: f32,
    f_swing: f32,
    fsteps: Steps,
    asteps: Steps,
    /// Its level, with the mix.
    gain: f32,
}

/// The settings for a chunk.
struct Set {
    ch: [ChanSet; 2],
    key_track: f32,
    /// Octaves on every cutoff from the brilliance and the LFO.
    octaves: f32,
    touch_cutoff: f32,
    after_cutoff: f32,
    /// Semitones of vibrato for every voice, from the LFO.
    vibrato: f32,
    /// More vibrato at full aftertouch, in semitones.
    after_vibrato: f32,
    lfo: f32,
    bend: f32,
    vintage: f32,
    glide: Glide,
}

/// One channel of a voice.
#[derive(Clone, Copy, Debug)]
struct Chan {
    saw: Osc,
    pulse: Osc,
    drift: Drift,
    noise: Noise,
    hp: [Svf; 2],
    lp: [Svf; 2],
    fenv: Analog,
    amp: Analog,
}

impl Chan {
    fn new(seed: u32) -> Chan {
        Chan {
            saw: Osc::new(Wave::Saw),
            pulse: Osc::new(Wave::Pulse),
            drift: Drift::new(seed),
            noise: Noise::new(seed.wrapping_mul(0x9e37_79b9) | 1),
            hp: [Svf::plain(); 2],
            lp: [Svf::plain(); 2],
            fenv: Analog::IDLE,
            amp: Analog::IDLE,
        }
    }

    fn reset(&mut self) {
        for f in self.hp.iter_mut().chain(self.lp.iter_mut()) {
            f.reset();
        }
        self.fenv = Analog::IDLE;
        self.amp = Analog::IDLE;
    }

    fn active(&self) -> bool {
        self.amp.active()
    }

    /// Runs the channel over a chunk at `pitch` (semitones, with the
    /// vibrato and bend in), adding its output to `out`. `press` is the
    /// aftertouch and `velocity` the strike, 0 to 1.
    #[allow(clippy::too_many_arguments)]
    #[inline]
    fn chunk(
        &mut self,
        s: &ChanSet,
        set: &Set,
        pitch: f32,
        velocity: f32,
        press: f32,
        timbre: f32,
        rate: f32,
        out: &mut [f32],
    ) {
        let n = out.len() as f32;
        let cents = self.drift.next(set.vintage, n, rate);
        let f = hz(pitch + s.tune + cents / 100.0);
        let dt = (f / rate).min(0.5);
        self.saw.dt = dt;
        self.pulse.dt = dt;
        self.pulse.width = s.width;
        // The filters, once a chunk: the key, the envelope between its
        // two levels, the touch, the brilliance and the LFO.
        let octaves =
            set.key_track * (pitch - 60.0) / 12.0 + s.f_initial + s.f_swing * self.fenv.level
                - set.touch_cutoff * (1.0 - velocity)
                + set.after_cutoff * press
                + timbre
                + set.octaves;
        let scale = octaves.clamp(-10.0, 10.0).exp2();
        let stages = if s.four_pole { 2 } else { 1 };
        if s.hp_on {
            for h in &mut self.hp[..stages] {
                h.set(SvfOut::Morph(1.0), s.hp * scale, s.hp_res, 1.0, rate);
            }
        }
        for l in &mut self.lp[..stages] {
            l.set(SvfOut::Morph(0.0), s.lp * scale, s.lp_res, 1.0, rate);
        }
        let gain = s.gain;
        for o in out.iter_mut() {
            let a = self.amp.next(&s.asteps);
            self.fenv.next(&s.fsteps);
            if !self.amp.active() {
                break;
            }
            let saw = self.saw.tick(None);
            let mut x = 0.0;
            if s.saw {
                x += saw;
            }
            if s.square {
                x += self.pulse.tick(None);
            }
            if s.noise > 0.0 {
                x += s.noise * self.noise.tick();
            }
            if s.hp_on {
                for h in &mut self.hp[..stages] {
                    x = h.run(x);
                }
            }
            for l in &mut self.lp[..stages] {
                x = l.run(x);
            }
            if s.sine > 0.0 {
                x += s.sine * (self.saw.phase * TAU).sin();
            }
            *o += x * a * gain;
        }
    }
}

/// One voice: two channels on one note.
#[derive(Clone, Copy, Debug)]
struct Voice {
    on: bool,
    note: u8,
    /// The pitch now, gliding towards `note`.
    pitch: f32,
    velocity: f32,
    /// The aftertouch, 0 to 1, and its smoothed value.
    press: f32,
    press_now: f32,
    /// The note's own tuning (semitones) and timbre (octaves on the
    /// filters), from note expression.
    tune: f32,
    timbre: f32,
    ch: [Chan; 2],
}

impl Voice {
    fn new(i: usize) -> Voice {
        let seed = (i as u32 + 1) * 7919;
        Voice {
            on: false,
            note: 60,
            pitch: 60.0,
            velocity: 1.0,
            press: 0.0,
            press_now: 0.0,
            tune: 0.0,
            timbre: 0.0,
            ch: [Chan::new(seed), Chan::new(seed + 3)],
        }
    }
}

/// The synth on the audio thread.
pub struct Eighty {
    params: Live,
    rate: f32,
    voices: Vec<Voice>,
    alloc: Allocator,
    actions: Vec<Action>,
    bend: f32,
    /// The mod wheel, 0 to 1: more vibrato.
    wheel: f32,
    /// Pressure on the whole keyboard, 0 to 1.
    pressure: f32,
    /// The last note played, where a glide starts from.
    last_pitch: f32,
    lfo_phase: f32,
    /// The random shape's held value.
    lfo_held: f32,
    lfo_noise: Noise,
    pwm_phase: [f32; 2],
    ring_phase: f32,
    ring_env: Analog,
    chorus: Chorus,
    /// The output's DC blocker: the last input and output.
    dc: (f32, f32),
    mix: [f32; CHUNK],
}

impl Eighty {
    pub fn new(rate: u32, params: impl Into<Live>) -> Eighty {
        let rate = rate.max(1) as f32;
        Eighty {
            params: params.into(),
            rate,
            voices: (0..MAX_VOICES).map(Voice::new).collect(),
            alloc: Allocator::new(MAX_VOICES),
            actions: Vec::with_capacity(32),
            bend: 0.0,
            wheel: 0.0,
            pressure: 0.0,
            last_pitch: 60.0,
            lfo_phase: 0.0,
            lfo_held: 0.0,
            lfo_noise: Noise::new(0x80_80_80),
            pwm_phase: [0.0, 0.5],
            ring_phase: 0.0,
            ring_env: Analog::IDLE,
            chorus: Chorus::new(rate as u32),
            dc: (0.0, 0.0),
            mix: [0.0; CHUNK],
        }
    }

    fn settings(&self) -> VoiceSettings {
        VoiceSettings {
            notes: (self.params.read(at::VOICES) as usize).clamp(1, MAX_VOICES),
            unison: 1,
            priority: Priority::Last,
            legato: false,
        }
    }

    fn apply(&mut self) {
        let sens = self.params.read(at::TOUCH_LEVEL);
        let glide = self.params.read(at::GLIDE) > 0.0;
        for a in self.actions.drain(..) {
            match a {
                Action::Start {
                    voice,
                    note,
                    velocity,
                    fresh,
                } => {
                    let v = &mut self.voices[voice];
                    v.note = note;
                    v.velocity = velocity.clamp(1, 127) as f32 / 127.0;
                    v.velocity = 1.0 - sens + sens * v.velocity;
                    v.press = self.pressure;
                    v.tune = 0.0;
                    v.timbre = 0.0;
                    if fresh {
                        // The oscillators keep running where they were,
                        // as free-running ones do; the filters and the
                        // envelopes start clean.
                        for c in &mut v.ch {
                            c.reset();
                        }
                        v.press_now = self.pressure;
                    }
                    v.pitch = if glide { self.last_pitch } else { note as f32 };
                    self.last_pitch = note as f32;
                    for c in &mut v.ch {
                        c.amp.trigger();
                        c.fenv.trigger();
                    }
                    v.on = true;
                    self.ring_env.trigger();
                }
                Action::Retune { voice, note } => self.voices[voice].note = note,
                Action::Release { voice } => {
                    for c in &mut self.voices[voice].ch {
                        c.amp.release();
                        c.fenv.release();
                    }
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
            Kind::Pressure(v) => {
                self.pressure = v.min(127) as f32 / 127.0;
                for v in self.voices.iter_mut().filter(|v| v.on) {
                    v.press = self.pressure;
                }
            }
            Kind::KeyPressure { pitch, value } => {
                let p = value.min(127) as f32 / 127.0;
                for v in self.voices.iter_mut().filter(|v| v.on && v.note == pitch) {
                    v.press = p;
                }
            }
            Kind::NoteExpression {
                pitch, kind, value, ..
            } => {
                for v in self.voices.iter_mut().filter(|v| v.on && v.note == pitch) {
                    match kind {
                        Expression::Pressure => v.press = value.clamp(0.0, 1.0),
                        Expression::Tuning => v.tune = value.clamp(-48.0, 48.0),
                        // Half a turn of brightness is two octaves.
                        Expression::Timbre => v.timbre = (value.clamp(0.0, 1.0) - 0.5) * 4.0,
                    }
                }
            }
            Kind::Control { number: 1, value } => self.wheel = value.min(127) as f32 / 127.0,
            Kind::Control { number: 64, value } => self.alloc.pedal(value >= 64, &mut self.actions),
            Kind::Control { number: 120, .. } | Kind::AllNotesOff => {
                self.alloc.all_off(&mut self.actions);
            }
            _ => {}
        }
        self.apply();
    }

    /// The sub-oscillator's value for a chunk, −1 to 1, advanced by
    /// `samples`.
    fn lfo(&mut self, shape: u32, speed: f32, samples: f32) -> f32 {
        let old = self.lfo_phase;
        self.lfo_phase = (old + speed * samples / self.rate).fract();
        if self.lfo_phase < old {
            self.lfo_held = self.lfo_noise.tick();
        }
        let p = self.lfo_phase;
        match shape {
            1 => 2.0 * p - 1.0,
            2 => 1.0 - 2.0 * p,
            3 => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            4 => self.lfo_held,
            _ => (p * TAU).sin(),
        }
    }

    /// Reads the settings for a chunk of `n` samples.
    fn read(&mut self, n: f32) -> Set {
        let r = self.rate;
        let lfo = self.lfo(
            self.params.read(at::LFO_SHAPE) as u32,
            self.params.read(at::LFO_SPEED),
            n,
        );
        let p = &self.params;
        let mix = p.read(at::MIX);
        let mut chans = [None, None];
        for (c, chan) in chans.iter_mut().enumerate() {
            let g = |k: usize| p.read(ch_at(c, k));
            let speed = g(ch::PWM_SPEED);
            self.pwm_phase[c] = (self.pwm_phase[c] + speed * n / r).fract();
            let pwm = (self.pwm_phase[c] * TAU).sin();
            let f_initial = g(ch::F_INITIAL);
            let hp = g(ch::HP);
            let hp_res = g(ch::HP_RES);
            let side = if c == 0 { 1.0 - mix } else { mix };
            *chan = Some(ChanSet {
                saw: g(ch::SAW) >= 0.5,
                square: g(ch::SQUARE) >= 0.5,
                width: (g(ch::WIDTH) + 0.45 * g(ch::PWM) * pwm).clamp(0.03, 0.97),
                noise: g(ch::NOISE),
                sine: g(ch::SINE),
                tune: g(ch::TUNE) / 100.0,
                hp,
                hp_res,
                hp_on: hp > 22.0 || hp_res > 0.01,
                lp: g(ch::LP),
                lp_res: g(ch::LP_RES),
                four_pole: g(ch::SLOPE) >= 0.5,
                f_initial,
                f_swing: g(ch::F_ATTACK_LEVEL) - f_initial,
                fsteps: Steps::new(
                    Shape::adr(0.0, 1.0, g(ch::F_ATTACK), g(ch::F_DECAY), g(ch::F_RELEASE)),
                    r,
                ),
                asteps: Steps::new(
                    Shape::adsr(g(ch::ATTACK), g(ch::DECAY), g(ch::SUSTAIN), g(ch::RELEASE)),
                    r,
                ),
                gain: g(ch::LEVEL) * (2.0 * side).min(1.0),
            });
        }
        Set {
            ch: [chans[0].unwrap(), chans[1].unwrap()],
            key_track: p.read(at::KEY_TRACK),
            octaves: p.read(at::BRILLIANCE) + p.read(at::LFO_FILTER) * lfo,
            touch_cutoff: p.read(at::TOUCH_CUTOFF),
            after_cutoff: p.read(at::AFTER_CUTOFF),
            // The wheel adds up to half a semitone of vibrato.
            vibrato: (p.read(at::LFO_PITCH) + 0.5 * self.wheel) * lfo,
            after_vibrato: p.read(at::AFTER_VIBRATO) * lfo,
            lfo,
            bend: self.bend * p.read(at::BEND),
            vintage: p.read(at::VINTAGE),
            glide: Glide {
                mode: GlideMode::Time,
                seconds: p.read(at::GLIDE),
            },
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
        let n = to - from;
        let set = self.read(n as f32);
        let r = self.rate;
        let p = &self.params;
        let after_level = p.read(at::AFTER_LEVEL);
        // The aftertouch is smoothed over about 20 ms so it never steps.
        let press_k = 1.0 - (-(n as f32) / (0.02 * r)).exp();
        let mix = &mut self.mix[..n];
        mix.iter_mut().for_each(|v| *v = 0.0);
        let mut sounding = false;
        for (i, v) in self.voices.iter_mut().enumerate() {
            if !v.on {
                continue;
            }
            v.pitch = set.glide.step(v.pitch, v.note as f32, n as f32, r);
            v.press_now += (v.press - v.press_now) * press_k;
            let press = v.press_now;
            let pitch = v.pitch + v.tune + set.bend + set.vibrato + set.after_vibrato * press;
            let gain = 1.0 + after_level * press;
            let mut out = [0.0f32; CHUNK];
            for c in 0..2 {
                if set.ch[c].gain > 0.0 && v.ch[c].active() {
                    v.ch[c].chunk(
                        &set.ch[c],
                        &set,
                        pitch,
                        v.velocity,
                        press,
                        v.timbre,
                        r,
                        &mut out[..n],
                    );
                }
            }
            if !v.ch[0].active() && !v.ch[1].active() {
                v.on = false;
                self.alloc.free(i);
                continue;
            }
            sounding = true;
            // The strike sets the level. Four voices at full stretch
            // reach the amplifier's soft limit; more lean on it.
            let g = v.velocity * gain * HEADROOM;
            for (m, o) in mix.iter_mut().zip(&out) {
                *m += o * g;
            }
        }
        // The ring modulator on the sum, its envelope sweeping the
        // speed; then the level, tremolo and the chorus.
        let ring_depth = p.read(at::RING_DEPTH);
        let ring_steps = Steps::new(
            Shape::adr(
                0.0,
                1.0,
                p.read(at::RING_ATTACK),
                p.read(at::RING_DECAY),
                p.read(at::RING_DECAY),
            ),
            r,
        );
        let ring_dt =
            p.read(at::RING_SPEED) * (p.read(at::RING_SWEEP) * self.ring_env.level).exp2() / r;
        let level = db(p.read(at::LEVEL));
        let tremolo = 1.0 - p.read(at::LFO_LEVEL) * 0.5 * (1.0 - set.lfo);
        self.chorus.mode = ChorusMode::from_index(p.read(at::CHORUS));
        let silent = !sounding && self.chorus.mode == ChorusMode::Off;
        let dc_k = 1.0 - TAU * 5.0 / r;
        for (k, &m) in mix.iter().enumerate() {
            self.ring_env.next(&ring_steps);
            let mut y = m;
            if ring_depth > 0.0 {
                self.ring_phase = (self.ring_phase + ring_dt).fract();
                let car = (self.ring_phase * TAU).sin();
                y *= 1.0 - ring_depth + ring_depth * car;
            }
            let y = soft(y) * level * tremolo;
            let out = y - self.dc.0 + dc_k * self.dc.1;
            self.dc = (y, out);
            if silent {
                continue;
            }
            let (l, rr) = self.chorus.run(out);
            left[from + k] += l;
            right[from + k] += rr;
        }
    }
}

impl Instrument for Eighty {
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

    fn synth() -> Eighty {
        let s = Eighty::new(RATE as u32, Arc::new(Params::new(params())));
        s.params.set(at::VINTAGE, 0.0);
        s.params.set(at::AFTER_VIBRATO, 0.0);
        s
    }

    fn render(s: &mut Eighty, events: &[Event], n: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        s.process(events, &mut l, &mut r);
        (l, r)
    }

    fn mono(s: &mut Eighty, events: &[Event], n: usize) -> Vec<f32> {
        render(s, events, n).0
    }

    /// The level of `hz` over `x`.
    fn level_at(x: &[f32], hz: f32) -> f32 {
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (i, &v) in x.iter().enumerate() {
            let p = TAU * hz * i as f32 / RATE;
            re += v * p.cos();
            im += v * p.sin();
        }
        (re * re + im * im).sqrt() * 2.0 / x.len() as f32
    }

    fn db_at(x: &[f32], hz: f32) -> f32 {
        20.0 * level_at(x, hz).max(1e-9).log10()
    }

    fn pressure(pitch: u8, value: u8) -> Event {
        Event {
            frame: 0,
            channel: 0,
            kind: Kind::KeyPressure { pitch, value },
        }
    }

    #[test]
    fn a_note_sounds_then_dies_away_on_both_sides() {
        let mut s = synth();
        let (l, r) = render(&mut s, &[Event::note_on(100, 0, 60, 100, 0)], 4800);
        assert_eq!(measure::peak(&l[..100]), 0.0);
        assert!(measure::peak(&l[100..]) > 0.05);
        assert_eq!(l, r, "no chorus: the same both sides");
        let (l, _) = render(&mut s, &[Event::note_off(0, 0, 60, 0)], 96_000);
        assert_eq!(measure::peak(&l[90_000..]), 0.0);
        assert!(s.voices.iter().all(|v| !v.on));
    }

    #[test]
    fn its_pitch_follows_the_note_and_the_channel_tune() {
        let mut s = synth();
        s.params.set(at::CH2 + ch::LEVEL, 0.0);
        s.params.set(at::CH1 + ch::LP, 18_000.0);
        s.params.set(at::CH1 + ch::F_ATTACK_LEVEL, 0.0);
        s.params.set(at::CH1 + ch::SAW, 0.0);
        s.params.set(at::CH1 + ch::SINE, 1.0);
        let x = mono(&mut s, &[Event::note_on(0, 0, 69, 127, 0)], 48_000);
        let f = measure::ring_hz(&x, RATE);
        assert!((f - 440.0).abs() < 2.0, "{f}");
        // Channel 1 a quarter tone up.
        s.params.set(at::CH1 + ch::TUNE, 50.0);
        let x = mono(&mut s, &[], 48_000);
        let f = measure::ring_hz(&x, RATE);
        assert!((f - 452.9).abs() < 2.5, "{f}");
    }

    #[test]
    fn eight_voices_play_at_once_and_the_ninth_steals_the_oldest() {
        let mut s = synth();
        let events: Vec<Event> = (0..8)
            .map(|i| Event::note_on(0, 0, 48 + i, 100, i as u32))
            .collect();
        mono(&mut s, &events, 4800);
        assert_eq!(s.voices.iter().filter(|v| v.on).count(), 8);
        // The ninth note takes a spare voice, and the oldest note is let
        // go into its release so eight are held.
        mono(&mut s, &[Event::note_on(0, 0, 72, 100, 9)], 480);
        assert_eq!(s.alloc.note_of(8), Some(72));
        assert!(s.voices[0].ch[0].amp.releasing());
        assert!(!s.voices[1].ch[0].amp.releasing());
        // Fewer voices: the setting limits them.
        let mut s = synth();
        s.params.set(at::VOICES, 2.0);
        mono(&mut s, &events, 4800);
        let held: Vec<_> = (0..MAX_VOICES).filter_map(|i| s.alloc.note_of(i)).collect();
        assert!(held.len() <= 8);
        assert!(held.contains(&54) && held.contains(&55));
    }

    #[test]
    fn the_filter_envelope_goes_from_the_initial_to_the_attack_level_and_back() {
        let mut s = synth();
        s.params.set(at::CH2 + ch::LEVEL, 0.0);
        s.params.set(at::CH1 + ch::LP, 300.0);
        s.params.set(at::CH1 + ch::LP_RES, 0.0);
        s.params.set(at::CH1 + ch::F_INITIAL, 0.0);
        s.params.set(at::CH1 + ch::F_ATTACK_LEVEL, 3.0);
        s.params.set(at::CH1 + ch::F_ATTACK, 0.3);
        s.params.set(at::CH1 + ch::F_DECAY, 0.3);
        s.params.set(at::CH1 + ch::ATTACK, 0.001);
        s.params.set(at::CH1 + ch::SUSTAIN, 1.0);
        s.params.set(at::TOUCH_CUTOFF, 0.0);
        let x = mono(&mut s, &[Event::note_on(0, 0, 48, 127, 0)], 48_000);
        // Brightness: the 5th harmonic against the fundamental, in
        // tenths of a second.
        let f0 = hz(48.0);
        let bright =
            |from: usize, to: usize| db_at(&x[from..to], 5.0 * f0) - db_at(&x[from..to], f0);
        let start = bright(0, 4800);
        let peak = bright(9600, 14_400);
        let later = bright(43_200, 48_000);
        assert!(peak > start + 6.0, "start {start} peak {peak}");
        assert!(later < peak - 6.0, "peak {peak} later {later}");
    }

    #[test]
    fn velocity_and_aftertouch_set_the_level_and_the_brightness() {
        let f0 = hz(48.0);
        let bright = |x: &[f32]| db_at(x, 5.0 * f0) - db_at(x, f0);
        let hit = |velocity: u8| {
            let mut s = synth();
            s.params.set(at::CH2 + ch::LEVEL, 0.0);
            s.params.set(at::CH1 + ch::F_ATTACK_LEVEL, 0.0);
            s.params.set(at::CH1 + ch::LP, 2000.0);
            s.params.set(at::TOUCH_LEVEL, 0.8);
            s.params.set(at::TOUCH_CUTOFF, 3.0);
            mono(&mut s, &[Event::note_on(0, 0, 48, velocity, 0)], 24_000)
        };
        let soft_hit = hit(30);
        let hard = hit(127);
        assert!(measure::peak(&hard) > 2.0 * measure::peak(&soft_hit));
        assert!(bright(&hard[12_000..]) > bright(&soft_hit[12_000..]) + 6.0);
        // Pressing the key harder afterwards opens it up and lifts it.
        let mut s = synth();
        s.params.set(at::CH2 + ch::LEVEL, 0.0);
        s.params.set(at::CH1 + ch::F_ATTACK_LEVEL, 0.0);
        s.params.set(at::CH1 + ch::LP, 600.0);
        s.params.set(at::TOUCH_CUTOFF, 0.0);
        s.params.set(at::AFTER_LEVEL, 1.0);
        s.params.set(at::AFTER_CUTOFF, 3.0);
        let before = mono(&mut s, &[Event::note_on(0, 0, 48, 127, 0)], 24_000);
        let after = mono(&mut s, &[pressure(48, 127)], 24_000);
        assert!(measure::peak(&after[12_000..]) > 1.5 * measure::peak(&before[12_000..]));
        let (b, a) = (bright(&before[12_000..]), bright(&after[12_000..]));
        assert!(a > b + 6.0, "{b} {a}");
        // Pressure on another key does nothing to this one.
        let other = mono(&mut s, &[pressure(50, 0)], 24_000);
        assert!((measure::peak(&other[12_000..]) - measure::peak(&after[12_000..])).abs() < 0.05);
    }

    #[test]
    fn a_notes_pressure_line_reaches_its_voice() {
        let mut s = synth();
        s.params.set(at::CH2 + ch::LEVEL, 0.0);
        s.params.set(at::CH1 + ch::F_ATTACK_LEVEL, 0.0);
        s.params.set(at::CH1 + ch::LP, 400.0);
        s.params.set(at::AFTER_LEVEL, 1.0);
        s.params.set(at::AFTER_CUTOFF, 0.0);
        let before = mono(&mut s, &[Event::note_on(0, 0, 48, 100, 3)], 24_000);
        let e = Event {
            frame: 0,
            channel: 0,
            kind: Kind::NoteExpression {
                id: 3,
                pitch: 48,
                kind: Expression::Pressure,
                value: 1.0,
            },
        };
        let after = mono(&mut s, &[e], 24_000);
        assert!(measure::peak(&after[12_000..]) > 1.5 * measure::peak(&before[12_000..]));
    }

    #[test]
    fn the_ring_modulator_puts_sidebands_either_side_of_the_note() {
        let mut s = synth();
        s.params.set(at::CH2 + ch::LEVEL, 0.0);
        s.params.set(at::CH1 + ch::SAW, 0.0);
        s.params.set(at::CH1 + ch::SINE, 1.0);
        s.params.set(at::CH1 + ch::F_ATTACK_LEVEL, 0.0);
        s.params.set(at::CH1 + ch::LP, 18_000.0);
        s.params.set(at::RING_DEPTH, 1.0);
        s.params.set(at::RING_SPEED, 100.0);
        s.params.set(at::RING_SWEEP, 0.0);
        let x = mono(&mut s, &[Event::note_on(0, 0, 69, 127, 0)], 48_000);
        let x = &x[24_000..];
        let side = db_at(x, 540.0).max(db_at(x, 340.0));
        assert!(side > db_at(x, 440.0) + 10.0, "{side}");
    }

    #[test]
    fn the_chorus_makes_the_sides_differ_and_the_lfo_gives_tremolo() {
        let mut s = synth();
        s.params.set(at::CHORUS, 1.0);
        let (l, r) = render(&mut s, &[Event::note_on(0, 0, 60, 100, 0)], 24_000);
        let diff = l
            .iter()
            .zip(&r)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(diff > 0.01, "{diff}");
        let mut s = synth();
        s.params.set(at::CH1 + ch::ATTACK, 0.001);
        s.params.set(at::CH2 + ch::ATTACK, 0.001);
        s.params.set(at::CH1 + ch::SUSTAIN, 1.0);
        s.params.set(at::CH2 + ch::SUSTAIN, 1.0);
        s.params.set(at::LFO_SPEED, 2.0);
        s.params.set(at::LFO_LEVEL, 1.0);
        let x = mono(&mut s, &[Event::note_on(0, 0, 60, 100, 0)], 48_000);
        // Half a cycle apart, a peak and a trough.
        let a = measure::peak(&x[6_000..7_000]);
        let b = measure::peak(&x[18_000..19_000]);
        assert!(a.max(b) > 3.0 * a.min(b), "{a} {b}");
    }

    #[test]
    fn glide_slides_from_the_last_note() {
        let mut s = synth();
        s.params.set(at::CH2 + ch::LEVEL, 0.0);
        s.params.set(at::CH1 + ch::SAW, 0.0);
        s.params.set(at::CH1 + ch::SINE, 1.0);
        s.params.set(at::CH1 + ch::LP, 18_000.0);
        s.params.set(at::CH1 + ch::F_ATTACK_LEVEL, 0.0);
        s.params.set(at::GLIDE, 0.5);
        mono(&mut s, &[Event::note_on(0, 0, 57, 100, 0)], 4800);
        let x = mono(
            &mut s,
            &[
                Event::note_off(0, 0, 57, 0),
                Event::note_on(0, 0, 69, 100, 1),
            ],
            4800,
        );
        let early = measure::ring_hz(&x, RATE);
        assert!(early < 400.0, "{early}");
        let x = mono(&mut s, &[], 96_000);
        let late = measure::ring_hz(&x, RATE);
        assert!((late - 440.0).abs() < 3.0, "{late}");
    }

    #[test]
    fn the_twenty_four_db_slope_is_steeper() {
        let bright = |four_pole: f64| {
            let mut s = synth();
            s.params.set(at::CH2 + ch::LEVEL, 0.0);
            s.params.set(at::CH1 + ch::LP, 200.0);
            s.params.set(at::CH1 + ch::LP_RES, 0.0);
            s.params.set(at::CH1 + ch::F_ATTACK_LEVEL, 0.0);
            s.params.set(at::CH1 + ch::SLOPE, four_pole);
            s.params.set(at::TOUCH_CUTOFF, 0.0);
            let x = mono(&mut s, &[Event::note_on(0, 0, 48, 127, 0)], 48_000);
            let x = &x[24_000..];
            db_at(x, hz(48.0) * 8.0) - db_at(x, hz(48.0))
        };
        let two = bright(0.0);
        let four = bright(1.0);
        assert!(four < two - 10.0, "12 dB {two}, 24 dB {four}");
    }

    /// A fixed sound renders to a known spectrum, so a later change to
    /// the toolkit can't alter it unnoticed.
    #[test]
    fn a_fixed_patch_renders_to_its_known_spectrum() {
        let b = crate::Builtin::new(crate::Kind::Eighty);
        b.set_sound("Brass").unwrap();
        let mut s = Eighty::new(RATE as u32, Live::new(b.params().clone()));
        s.params.set(at::VINTAGE, 0.0);
        // No detune, so the channels don't beat over the window.
        s.params.set(at::CH2 + ch::TUNE, 0.0);
        let x = mono(&mut s, &[Event::note_on(0, 0, 48, 100, 0)], 48_000);
        let x = &x[24_000..];
        let f0 = hz(48.0);
        let got: Vec<f32> = (1..=8).map(|h| db_at(x, f0 * h as f32)).collect();
        let known = [-20.3, -31.1, -39.4, -46.6, -53.5, -58.1, -62.8, -65.0];
        for (h, (g, k)) in got.iter().zip(&known).enumerate() {
            assert!((g - k).abs() < 1.5, "harmonic {}: {got:?}", h + 1);
        }
    }

    /// Eight voices with both channels, noise and the chorus: the
    /// budget is a modest share of one core (release builds only).
    #[test]
    #[cfg_attr(debug_assertions, ignore)]
    fn eight_voices_fit_the_budget() {
        let mut s = synth();
        s.params.set(at::VINTAGE, 0.5);
        s.params.set(at::CHORUS, 3.0);
        s.params.set(at::RING_DEPTH, 0.5);
        for c in 0..2 {
            s.params.set(ch_at(c, ch::SAW), 1.0);
            s.params.set(ch_at(c, ch::SQUARE), 1.0);
            s.params.set(ch_at(c, ch::NOISE), 0.2);
            s.params.set(ch_at(c, ch::SINE), 0.3);
            s.params.set(ch_at(c, ch::HP), 150.0);
            s.params.set(ch_at(c, ch::RELEASE), 8.0);
        }
        let events: Vec<Event> = (0..8)
            .map(|i| Event::note_on(0, 0, 48 + i as u8, 100, i))
            .collect();
        let seconds = 2.0;
        let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
        let start = std::time::Instant::now();
        s.process(&events, &mut l, &mut r);
        for _ in 0..(48_000.0 * seconds / 512.0) as usize {
            s.process(&[], &mut l, &mut r);
        }
        let used = start.elapsed().as_secs_f64() / seconds;
        eprintln!("eight voices: {:.1}% of one core", used * 100.0);
        assert!(used < 0.25, "{:.1}% of one core", used * 100.0);
    }
}
