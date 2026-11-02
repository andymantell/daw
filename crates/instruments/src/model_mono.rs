// SPDX-License-Identifier: GPL-3.0-or-later
//! Model Mono: a three-oscillator ladder monosynth built the way the
//! 1970 American classic was (`docs/plans/classic-synths.md`, S1), for
//! bass and leads. Three free-running oscillators with six footages and
//! six shapes (triangle, shark tooth, saw, square, wide and narrow
//! pulse), the third doubling as a modulation source at a fixed rate;
//! white or pink noise; a mixer that overdrives as its levels rise; the
//! 24 dB transistor ladder with emphasis, key tracking in thirds and
//! its own contour; a loudness contour; both contours attack, decay and
//! sustain, with a switch that makes the release take the decay's
//! time; glide at a constant rate; low, high or last note priority;
//! single or multiple triggering; and the mod wheel to pitch and
//! filter from oscillator 3 or noise.
//!
//! Everything nonlinear is where the circuit has it: the mixer's
//! clipping, the ladder's saturating stages (oversampled), the
//! amplifier's soft limit. The oscillators drift a little, each on its
//! own, by the vintage amount. One note at a time; the output is the
//! same on both sides.

use daw_engine::midi::{Event, Instrument, Kind};

use crate::dsp::{Noise, db, hz};
use crate::params::{Live, Param, Unit};
use crate::vintage::{
    Action, Allocator, Analog, Drift, Glide, GlideMode, Ladder, Osc, Priority, Shape, Steps,
    VoiceSettings, Wave, soft,
};

/// The oscillators' shapes, as the six-position switch had them.
const WAVES: &[&str] = &[
    "Triangle",
    "Shark tooth",
    "Saw",
    "Square",
    "Wide pulse",
    "Narrow pulse",
];
/// The footages: "Lo" is an octave under 32', for oscillator 3 as a
/// slow modulator.
const RANGES: &[&str] = &["Lo", "32'", "16'", "8'", "4'", "2'"];
/// Semitones from 8' for each range.
const RANGE_SEMIS: [f32; 6] = [-36.0, -24.0, -12.0, 0.0, 12.0, 24.0];
/// Oscillator 3 can leave the keyboard and run at a fixed rate.
const OSC3_MODES: &[&str] = &["Follows the keys", "Fixed rate (LFO)"];
const NOISE_COLOURS: &[&str] = &["White", "Pink"];
/// The filter's keyboard tracking switches: none, a third, two thirds.
const KEY_TRACK: &[&str] = &["Off", "1/3", "2/3", "Full"];
const TRIGGER: &[&str] = &["Single (legato)", "Every note"];

/// Settings by index; the order of [`params`].
pub mod at {
    pub const OSC1_WAVE: usize = 0;
    pub const OSC1_RANGE: usize = 1;
    pub const OSC1_LEVEL: usize = 2;
    pub const OSC2_WAVE: usize = 3;
    pub const OSC2_RANGE: usize = 4;
    pub const OSC2_TUNE: usize = 5;
    pub const OSC2_LEVEL: usize = 6;
    pub const OSC3_WAVE: usize = 7;
    pub const OSC3_RANGE: usize = 8;
    pub const OSC3_TUNE: usize = 9;
    pub const OSC3_LEVEL: usize = 10;
    pub const OSC3_KEYS: usize = 11;
    pub const NOISE_LEVEL: usize = 12;
    pub const NOISE_COLOUR: usize = 13;
    pub const OVERDRIVE: usize = 14;
    pub const CUTOFF: usize = 15;
    pub const EMPHASIS: usize = 16;
    pub const CONTOUR: usize = 17;
    pub const KEY_TRACK: usize = 18;
    pub const F_ATTACK: usize = 19;
    pub const F_DECAY: usize = 20;
    pub const F_SUSTAIN: usize = 21;
    pub const ATTACK: usize = 22;
    pub const DECAY: usize = 23;
    pub const SUSTAIN: usize = 24;
    pub const DECAY_RELEASE: usize = 25;
    pub const MOD_MIX: usize = 26;
    pub const MOD_AMOUNT: usize = 27;
    pub const OSC_MOD: usize = 28;
    pub const FILTER_MOD: usize = 29;
    pub const GLIDE: usize = 30;
    pub const PRIORITY: usize = 31;
    pub const TRIGGER: usize = 32;
    pub const BEND: usize = 33;
    pub const VELOCITY: usize = 34;
    pub const LEVEL: usize = 35;
    pub const VINTAGE: usize = 36;
}

/// The synth's settings, in the order of [`at`]. Grouped for showing by
/// [`GROUPS`].
pub fn params() -> Vec<Param> {
    use Unit::*;
    vec![
        Param::new("osc1_wave", "Osc 1 shape", 0.0, 5.0, 2.0, Choice).choices(WAVES),
        Param::new("osc1_range", "Osc 1 range", 0.0, 5.0, 3.0, Choice).choices(RANGES),
        Param::new("osc1_level", "Osc 1 level", 0.0, 1.0, 0.8, Amount),
        Param::new("osc2_wave", "Osc 2 shape", 0.0, 5.0, 2.0, Choice).choices(WAVES),
        Param::new("osc2_range", "Osc 2 range", 0.0, 5.0, 3.0, Choice).choices(RANGES),
        Param::new("osc2_tune", "Osc 2 tune", -7.0, 7.0, 0.05, Semitones),
        Param::new("osc2_level", "Osc 2 level", 0.0, 1.0, 0.6, Amount),
        Param::new("osc3_wave", "Osc 3 shape", 0.0, 5.0, 3.0, Choice).choices(WAVES),
        Param::new("osc3_range", "Osc 3 range", 0.0, 5.0, 2.0, Choice).choices(RANGES),
        Param::new("osc3_tune", "Osc 3 tune", -7.0, 7.0, 0.0, Semitones),
        Param::new("osc3_level", "Osc 3 level", 0.0, 1.0, 0.0, Amount),
        Param::new("osc3_keys", "Osc 3 pitch", 0.0, 1.0, 0.0, Choice).choices(OSC3_MODES),
        Param::new("noise_level", "Noise level", 0.0, 1.0, 0.0, Amount),
        Param::new("noise_colour", "Noise colour", 0.0, 1.0, 0.0, Choice).choices(NOISE_COLOURS),
        Param::new("overdrive", "Mixer overdrive", 0.0, 1.0, 0.25, Amount),
        Param::new("cutoff", "Cutoff", 20.0, 18_000.0, 900.0, Hz).log(),
        Param::new("emphasis", "Emphasis", 0.0, 1.0, 0.3, Amount),
        Param::new("contour", "Contour amount", 0.0, 6.0, 2.5, Octaves),
        Param::new("key_track", "Keyboard tracking", 0.0, 3.0, 1.0, Choice).choices(KEY_TRACK),
        Param::new("f_attack", "Filter attack", 0.001, 10.0, 0.004, Seconds).log(),
        Param::new("f_decay", "Filter decay", 0.004, 10.0, 0.4, Seconds).log(),
        Param::new("f_sustain", "Filter sustain", 0.0, 1.0, 0.2, Amount),
        Param::new("attack", "Attack", 0.001, 10.0, 0.003, Seconds).log(),
        Param::new("decay", "Decay", 0.004, 10.0, 0.5, Seconds).log(),
        Param::new("sustain", "Sustain", 0.0, 1.0, 0.8, Amount),
        Param::new(
            "decay_release",
            "Release takes the decay time",
            0.0,
            1.0,
            1.0,
            Switch,
        ),
        Param::new(
            "mod_mix",
            "Mod source: osc 3 to noise",
            0.0,
            1.0,
            0.0,
            Amount,
        ),
        Param::new("mod_amount", "Mod amount", 0.0, 1.0, 0.0, Amount),
        Param::new("osc_mod", "Mod to pitch", 0.0, 1.0, 0.0, Switch),
        Param::new("filter_mod", "Mod to filter", 0.0, 1.0, 0.0, Switch),
        Param::new("glide", "Glide time per octave", 0.0, 2.0, 0.0, Seconds),
        Param::new("priority", "Note priority", 0.0, 2.0, 2.0, Choice).choices(Priority::NAMES),
        Param::new("trigger", "Trigger", 0.0, 1.0, 0.0, Choice).choices(TRIGGER),
        Param::new("bend", "Bend range", 0.0, 24.0, 2.0, Semitones),
        Param::new("velocity", "Velocity", 0.0, 1.0, 0.3, Amount),
        Param::new("level", "Level", -36.0, 6.0, -8.0, Db),
        Param::new("vintage", "Vintage drift", 0.0, 1.0, 0.4, Amount),
    ]
}

/// The settings in groups, for the controls: (heading, first, last).
pub const GROUPS: &[(&str, usize, usize)] = &[
    ("Oscillators", at::OSC1_WAVE, at::OSC3_KEYS),
    ("Mixer", at::NOISE_LEVEL, at::OVERDRIVE),
    ("Filter", at::CUTOFF, at::F_SUSTAIN),
    ("Loudness", at::ATTACK, at::DECAY_RELEASE),
    ("Modulation", at::MOD_MIX, at::FILTER_MOD),
    ("Playing", at::GLIDE, at::VINTAGE),
];

/// Sounds made here, as settings over the defaults: bass, leads and
/// keys, for the content browser and the assistant.
pub fn sounds() -> Vec<crate::Sound> {
    use crate::Sound;
    vec![
        Sound {
            name: "Fat bass",
            settings: &[
                ("osc1_wave", 2.0),
                ("osc1_range", 2.0),
                ("osc2_wave", 2.0),
                ("osc2_range", 2.0),
                ("osc2_tune", 0.08),
                ("osc3_wave", 3.0),
                ("osc3_range", 1.0),
                ("osc3_level", 0.5),
                ("overdrive", 0.5),
                ("cutoff", 320.0),
                ("emphasis", 0.35),
                ("contour", 2.5),
                ("f_decay", 0.35),
                ("f_sustain", 0.1),
                ("decay", 0.6),
                ("sustain", 0.7),
                ("key_track", 1.0),
            ],
        },
        Sound {
            name: "Round bass",
            settings: &[
                ("osc1_wave", 0.0),
                ("osc1_range", 2.0),
                ("osc1_level", 0.9),
                ("osc2_wave", 3.0),
                ("osc2_range", 1.0),
                ("osc2_level", 0.4),
                ("osc3_level", 0.0),
                ("overdrive", 0.35),
                ("cutoff", 260.0),
                ("emphasis", 0.15),
                ("contour", 1.5),
                ("f_decay", 0.25),
                ("f_sustain", 0.3),
                ("decay", 0.8),
                ("sustain", 0.6),
                ("key_track", 2.0),
            ],
        },
        Sound {
            name: "Growl bass",
            settings: &[
                ("osc1_wave", 2.0),
                ("osc1_range", 1.0),
                ("osc1_level", 1.0),
                ("osc2_wave", 1.0),
                ("osc2_range", 2.0),
                ("osc2_tune", -0.1),
                ("osc2_level", 0.9),
                ("osc3_wave", 5.0),
                ("osc3_range", 2.0),
                ("osc3_tune", 0.12),
                ("osc3_level", 0.7),
                ("overdrive", 0.85),
                ("cutoff", 480.0),
                ("emphasis", 0.55),
                ("contour", 3.0),
                ("f_decay", 0.5),
                ("f_sustain", 0.25),
                ("decay", 0.5),
                ("sustain", 0.8),
                ("key_track", 1.0),
            ],
        },
        Sound {
            name: "Screaming lead",
            settings: &[
                ("osc1_wave", 2.0),
                ("osc1_range", 3.0),
                ("osc2_wave", 2.0),
                ("osc2_range", 3.0),
                ("osc2_tune", -0.12),
                ("osc2_level", 0.8),
                ("osc3_wave", 2.0),
                ("osc3_range", 3.0),
                ("osc3_tune", 0.15),
                ("osc3_level", 0.8),
                ("overdrive", 0.6),
                ("cutoff", 1800.0),
                ("emphasis", 0.45),
                ("contour", 3.0),
                ("f_decay", 0.8),
                ("f_sustain", 0.5),
                ("decay", 0.6),
                ("sustain", 0.9),
                ("key_track", 2.0),
                ("glide", 0.06),
                ("priority", 1.0),
            ],
        },
        Sound {
            name: "Vibrato lead",
            settings: &[
                ("osc1_wave", 3.0),
                ("osc1_range", 3.0),
                ("osc2_wave", 2.0),
                ("osc2_range", 4.0),
                ("osc2_tune", 0.06),
                ("osc2_level", 0.4),
                ("osc3_wave", 0.0),
                ("osc3_range", 0.0),
                ("osc3_tune", 5.2),
                ("osc3_keys", 1.0),
                ("osc3_level", 0.0),
                ("overdrive", 0.3),
                ("cutoff", 2400.0),
                ("emphasis", 0.25),
                ("contour", 2.0),
                ("f_attack", 0.02),
                ("f_decay", 0.6),
                ("f_sustain", 0.6),
                ("attack", 0.01),
                ("sustain", 0.9),
                ("key_track", 3.0),
                ("mod_amount", 0.12),
                ("osc_mod", 1.0),
                ("glide", 0.04),
            ],
        },
        Sound {
            name: "Brass",
            settings: &[
                ("osc1_wave", 2.0),
                ("osc1_range", 3.0),
                ("osc2_wave", 3.0),
                ("osc2_range", 3.0),
                ("osc2_tune", 0.04),
                ("osc2_level", 0.5),
                ("osc3_wave", 2.0),
                ("osc3_range", 2.0),
                ("osc3_level", 0.6),
                ("overdrive", 0.4),
                ("cutoff", 600.0),
                ("emphasis", 0.2),
                ("contour", 4.0),
                ("f_attack", 0.06),
                ("f_decay", 0.3),
                ("f_sustain", 0.45),
                ("attack", 0.03),
                ("sustain", 1.0),
                ("key_track", 2.0),
            ],
        },
        Sound {
            name: "Soft keys",
            settings: &[
                ("osc1_wave", 0.0),
                ("osc1_range", 3.0),
                ("osc2_wave", 3.0),
                ("osc2_range", 4.0),
                ("osc2_level", 0.25),
                ("osc3_level", 0.0),
                ("overdrive", 0.1),
                ("cutoff", 1100.0),
                ("emphasis", 0.1),
                ("contour", 2.0),
                ("f_decay", 0.3),
                ("f_sustain", 0.0),
                ("decay", 1.2),
                ("sustain", 0.3),
                ("key_track", 3.0),
                ("trigger", 1.0),
                ("velocity", 0.6),
            ],
        },
        Sound {
            name: "Wind",
            settings: &[
                ("osc1_level", 0.0),
                ("osc2_level", 0.0),
                ("osc3_level", 0.0),
                ("noise_level", 0.9),
                ("noise_colour", 1.0),
                ("overdrive", 0.0),
                ("cutoff", 700.0),
                ("emphasis", 0.75),
                ("contour", 1.5),
                ("f_attack", 1.5),
                ("f_decay", 2.0),
                ("f_sustain", 0.5),
                ("attack", 1.2),
                ("decay", 2.0),
                ("sustain", 0.8),
                ("key_track", 3.0),
                ("osc3_wave", 0.0),
                ("osc3_range", 0.0),
                ("osc3_tune", -4.0),
                ("osc3_keys", 1.0),
                ("mod_amount", 0.3),
                ("filter_mod", 1.0),
            ],
        },
    ]
}

/// Settings are read, and the filter set, this often.
const CHUNK: usize = 16;

/// An oscillator shape choice as a wave and a pulse width.
fn shape(choice: f32) -> (Wave, f32) {
    match choice as u32 {
        0 => (Wave::Triangle, 0.5),
        1 => (Wave::Shark, 0.5),
        3 => (Wave::Pulse, 0.5),
        4 => (Wave::Pulse, 0.25),
        5 => (Wave::Pulse, 0.1),
        _ => (Wave::Saw, 0.5),
    }
}

/// Oscillator 3's rate when it leaves the keyboard: each range doubles
/// it, and the tune knob (±7) spans three and a half octaves either
/// way, so "Lo" covers slow sweeps to vibrato and 8' reaches audio.
fn fixed_hz(range: u32, tune: f32) -> f32 {
    [1.0, 2.0, 4.0, 8.0, 16.0, 32.0][range.min(5) as usize] * (tune * 0.5).exp2()
}

/// Pink noise, near enough: three one-pole low-passes a decade apart
/// summed with gains a decade apart in power, for about −3 dB an
/// octave over the audio band, plus a little of the white noise for
/// the top.
#[derive(Clone, Copy, Debug, Default)]
struct Pink {
    s: [f32; 3],
    k: [f32; 3],
}

impl Pink {
    fn new(rate: f32) -> Pink {
        let k = |hz: f32| 1.0 - (-std::f32::consts::TAU * hz / rate).exp();
        Pink {
            s: [0.0; 3],
            k: [k(25.0), k(250.0), k(2500.0)],
        }
    }

    #[inline]
    fn tick(&mut self, white: f32) -> f32 {
        for i in 0..3 {
            self.s[i] += self.k[i] * (white - self.s[i]);
        }
        2.5 * (self.s[0] + 0.32 * self.s[1] + 0.1 * self.s[2] + 0.03 * white)
    }
}

/// The one voice.
#[derive(Clone, Copy, Debug)]
struct Voice {
    on: bool,
    note: u8,
    /// The pitch now, gliding towards `note`.
    pitch: f32,
    velocity: f32,
    osc: [Osc; 3],
    drift: [Drift; 3],
    amp: Analog,
    fenv: Analog,
    filter: Ladder,
    /// Oscillator 3's last sample, the modulation source.
    osc3: f32,
}

/// The synth on the audio thread.
pub struct ModelMono {
    params: Live,
    rate: f32,
    voice: Voice,
    alloc: Allocator,
    actions: Vec<Action>,
    bend: f32,
    /// The mod wheel, 0 to 1.
    wheel: f32,
    noise: Noise,
    pink: Pink,
    /// The output's DC blocker: the last input and output.
    dc: (f32, f32),
}

impl ModelMono {
    pub fn new(rate: u32, params: impl Into<Live>) -> ModelMono {
        let rate = rate.max(1) as f32;
        ModelMono {
            params: params.into(),
            rate,
            voice: Voice {
                on: false,
                note: 48,
                pitch: 48.0,
                velocity: 0.0,
                osc: [
                    Osc::new(Wave::Saw),
                    Osc::new(Wave::Saw),
                    Osc::new(Wave::Pulse),
                ],
                // The oscillators free-run, so they start apart.
                drift: [Drift::new(11), Drift::new(23), Drift::new(37)],
                amp: Analog::IDLE,
                fenv: Analog::IDLE,
                filter: Ladder::new(),
                osc3: 0.0,
            },
            alloc: Allocator::new(1),
            actions: Vec::with_capacity(8),
            bend: 0.0,
            wheel: 0.0,
            noise: Noise::new(0x5eed_1234),
            pink: Pink::new(rate),
            dc: (0.0, 0.0),
        }
    }

    fn settings(&self) -> VoiceSettings {
        VoiceSettings {
            notes: 1,
            unison: 1,
            priority: Priority::from_index(self.params.read(at::PRIORITY)),
            legato: self.params.read(at::TRIGGER) < 0.5,
        }
    }

    fn apply(&mut self) {
        let sens = self.params.read(at::VELOCITY);
        let v = &mut self.voice;
        for a in self.actions.drain(..) {
            match a {
                Action::Start {
                    note,
                    velocity,
                    fresh,
                    ..
                } => {
                    v.note = note;
                    v.velocity = 1.0 - sens + sens * velocity.clamp(1, 127) as f32 / 127.0;
                    if fresh {
                        // The oscillators keep running where they were,
                        // as free-running ones do; the filter starts
                        // clean.
                        v.filter.reset();
                        v.amp = Analog::IDLE;
                        v.fenv = Analog::IDLE;
                    }
                    if fresh && self.params.read(at::GLIDE) <= 0.0 {
                        v.pitch = note as f32;
                    }
                    v.amp.trigger();
                    v.fenv.trigger();
                    v.on = true;
                }
                Action::Retune { note, .. } => v.note = note,
                Action::Release { .. } => {
                    v.amp.release();
                    v.fenv.release();
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

    fn chunk(&mut self, from: usize, to: usize, left: &mut [f32], right: &mut [f32]) {
        if !self.voice.on {
            return;
        }
        let p = &self.params;
        let r = self.rate;
        let n = (to - from) as f32;
        let v = &mut self.voice;
        // Glide, at a constant rate: a wide jump takes longer.
        let glide = Glide {
            mode: GlideMode::Rate,
            seconds: p.read(at::GLIDE),
        };
        v.pitch = glide.step(v.pitch, v.note as f32, n, r);
        let vintage = p.read(at::VINTAGE);
        let bend = self.bend * p.read(at::BEND);
        let pitch = v.pitch + bend;
        // Each oscillator's shape, pitch and level.
        let waves = [
            shape(p.read(at::OSC1_WAVE)),
            shape(p.read(at::OSC2_WAVE)),
            shape(p.read(at::OSC3_WAVE)),
        ];
        let ranges = [
            p.read(at::OSC1_RANGE) as u32,
            p.read(at::OSC2_RANGE) as u32,
            p.read(at::OSC3_RANGE) as u32,
        ];
        let tunes = [0.0, p.read(at::OSC2_TUNE), p.read(at::OSC3_TUNE)];
        let levels = [
            p.read(at::OSC1_LEVEL),
            p.read(at::OSC2_LEVEL),
            p.read(at::OSC3_LEVEL),
        ];
        let osc3_fixed = p.read(at::OSC3_KEYS) >= 0.5;
        let mut dts = [0.0f32; 3];
        for i in 0..3 {
            let (wave, width) = waves[i];
            v.osc[i].wave = wave;
            v.osc[i].width = width;
            let cents = v.drift[i].next(vintage, n, r);
            let f = if i == 2 && osc3_fixed {
                fixed_hz(ranges[2], tunes[2]) * (cents / 1200.0).exp2()
            } else {
                hz(pitch + RANGE_SEMIS[ranges[i].min(5) as usize] + tunes[i] + cents / 100.0)
            };
            dts[i] = (f / r).min(0.5);
            v.osc[i].dt = dts[i];
        }
        let noise_level = p.read(at::NOISE_LEVEL);
        let pink = p.read(at::NOISE_COLOUR) >= 0.5;
        // The mixer: the sum clips softly, harder as its levels and the
        // overdrive rise. At no overdrive one oscillator at full level
        // is just clean; three at full are well into it.
        let mix_gain = 0.6 * 4f32.powf(p.read(at::OVERDRIVE));
        // Modulation: oscillator 3 or noise, by the wheel and the knob.
        let depth = (self.wheel + p.read(at::MOD_AMOUNT)).clamp(0.0, 1.0);
        let mod_mix = p.read(at::MOD_MIX);
        let to_pitch = if p.read(at::OSC_MOD) >= 0.5 && depth > 0.0 {
            // Full wheel is a fifth either way.
            depth * 7.0 / 12.0
        } else {
            0.0
        };
        let to_filter = if p.read(at::FILTER_MOD) >= 0.5 {
            depth * 3.0
        } else {
            0.0
        };
        let track = [0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0][(p.read(at::KEY_TRACK) as usize).min(3)];
        let contour = p.read(at::CONTOUR);
        let cutoff = p.read(at::CUTOFF);
        let emphasis = p.read(at::EMPHASIS);
        let release_on = p.read(at::DECAY_RELEASE) >= 0.5;
        let fsteps = Steps::new(
            Shape::ads(
                p.read(at::F_ATTACK),
                p.read(at::F_DECAY),
                p.read(at::F_SUSTAIN),
                release_on,
            ),
            r,
        );
        let asteps = Steps::new(
            Shape::ads(
                p.read(at::ATTACK),
                p.read(at::DECAY),
                p.read(at::SUSTAIN),
                release_on,
            ),
            r,
        );
        // The filter, once a chunk: the knob, the key, the contour and
        // the modulation as it stands.
        let m0 = (1.0 - mod_mix) * v.osc3 + mod_mix * self.noise.tick();
        let octaves = track * (pitch - 60.0) / 12.0 + contour * v.fenv.level + to_filter * m0;
        v.filter.set(cutoff * octaves.exp2(), emphasis, 1.0, 0.3, r);
        let gain = db(p.read(at::LEVEL)) * v.velocity;
        // The DC blocker, a one-pole high-pass at 5 Hz.
        let dc_k = 1.0 - std::f32::consts::TAU * 5.0 / r;
        for i in from..to {
            let a = v.amp.next(&asteps);
            v.fenv.next(&fsteps);
            if !v.amp.active() {
                v.on = false;
                self.alloc.free(0);
                break;
            }
            let white = self.noise.tick();
            let nz = if pink { self.pink.tick(white) } else { white };
            let m = (1.0 - mod_mix) * v.osc3 + mod_mix * nz;
            if to_pitch > 0.0 {
                let f = (m * to_pitch).exp2();
                v.osc[0].dt = (dts[0] * f).min(0.5);
                v.osc[1].dt = (dts[1] * f).min(0.5);
                if !osc3_fixed {
                    v.osc[2].dt = (dts[2] * f).min(0.5);
                }
            }
            let o1 = v.osc[0].tick(None);
            let o2 = v.osc[1].tick(None);
            let o3 = v.osc[2].tick(None);
            v.osc3 = o3;
            let sum = levels[0] * o1 + levels[1] * o2 + levels[2] * o3 + noise_level * nz;
            let mixed = soft(sum * mix_gain);
            let filtered = v.filter.run(mixed);
            // The amplifier's own soft limit, then the level.
            let y = soft(filtered * a) * gain;
            let out = y - self.dc.0 + dc_k * self.dc.1;
            self.dc = (y, out);
            left[i] += out;
            right[i] += out;
        }
    }
}

impl Instrument for ModelMono {
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

    fn synth() -> ModelMono {
        ModelMono::new(RATE as u32, Arc::new(Params::new(params())))
    }

    fn render(s: &mut ModelMono, events: &[Event], n: usize) -> Vec<f32> {
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        s.process(events, &mut l, &mut r);
        assert!(
            l.iter().zip(&r).all(|(a, b)| a == b),
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

    #[test]
    fn a_note_sounds_then_dies_away() {
        let mut s = synth();
        let x = render(&mut s, &[Event::note_on(100, 0, 36, 100, 0)], 4800);
        assert_eq!(measure::peak(&x[..100]), 0.0);
        assert!(measure::peak(&x[100..]) > 0.05);
        let x = render(&mut s, &[Event::note_off(0, 0, 36, 0)], 96_000);
        assert_eq!(measure::peak(&x[90_000..]), 0.0);
        assert!(!s.voice.on);
    }

    #[test]
    fn its_pitch_follows_the_note_and_the_ranges() {
        let mut s = synth();
        s.params.set(at::VINTAGE, 0.0);
        s.params.set(at::OSC2_LEVEL, 0.0);
        s.params.set(at::OSC2_TUNE, 0.0);
        s.params.set(at::CUTOFF, 18_000.0);
        s.params.set(at::CONTOUR, 0.0);
        s.params.set(at::OVERDRIVE, 0.0);
        let x = render(&mut s, &[Event::note_on(0, 0, 69, 127, 0)], 48_000);
        let f = measure::ring_hz(&x, RATE);
        assert!((f - 440.0).abs() < 8.0, "{f}");
        // 16' is an octave down.
        s.params.set(at::OSC1_RANGE, 2.0);
        let x = render(&mut s, &[], 48_000);
        let f = measure::ring_hz(&x, RATE);
        assert!((f - 220.0).abs() < 5.0, "{f}");
    }

    #[test]
    fn the_mixer_overdrives_as_it_is_pushed() {
        // A triangle alone has a weak third harmonic (a ninth of the
        // fundamental); clipping the mixer's sum adds to it.
        let third = |overdrive: f64| {
            let mut s = synth();
            s.params.set(at::VINTAGE, 0.0);
            s.params.set(at::OSC1_WAVE, 0.0);
            s.params.set(at::OSC1_LEVEL, 1.0);
            s.params.set(at::OSC2_LEVEL, 0.0);
            s.params.set(at::CUTOFF, 18_000.0);
            s.params.set(at::EMPHASIS, 0.0);
            s.params.set(at::CONTOUR, 0.0);
            s.params.set(at::OVERDRIVE, overdrive);
            let x = render(&mut s, &[Event::note_on(0, 0, 45, 127, 0)], 48_000);
            let f0 = hz(45.0);
            level_at(&x, 3.0 * f0) / level_at(&x, f0)
        };
        let (clean, hot) = (third(0.0), third(1.0));
        assert!(clean < 0.2, "{clean}");
        assert!(hot > 1.5 * clean, "{clean} {hot}");
        let mut s = synth();
        s.params.set(at::OVERDRIVE, 1.0);
        let x = render(&mut s, &[Event::note_on(0, 0, 36, 127, 0)], 48_000);
        assert!(x.iter().all(|v| v.is_finite() && v.abs() < 2.0));
    }

    #[test]
    fn emphasis_sings_and_the_contour_opens_the_filter() {
        let mut s = synth();
        s.params.set(at::VINTAGE, 0.0);
        s.params.set(at::OSC1_LEVEL, 0.0);
        s.params.set(at::OSC2_LEVEL, 0.0);
        s.params.set(at::NOISE_LEVEL, 0.02);
        s.params.set(at::CUTOFF, 1000.0);
        s.params.set(at::EMPHASIS, 1.0);
        s.params.set(at::CONTOUR, 0.0);
        s.params.set(at::KEY_TRACK, 0.0);
        s.params.set(at::SUSTAIN, 1.0);
        let x = render(&mut s, &[Event::note_on(0, 0, 60, 127, 0)], 96_000);
        let f = measure::ring_hz(&x[48_000..], RATE);
        assert!((f / 1000.0 - 1.0).abs() < 0.08, "sings at {f}");
        // With the contour, the start is brighter than the hold.
        let mut s = synth();
        s.params.set(at::VINTAGE, 0.0);
        s.params.set(at::CUTOFF, 200.0);
        s.params.set(at::CONTOUR, 4.0);
        s.params.set(at::F_DECAY, 0.1);
        s.params.set(at::F_SUSTAIN, 0.0);
        s.params.set(at::SUSTAIN, 1.0);
        let x = render(&mut s, &[Event::note_on(0, 0, 36, 127, 0)], 48_000);
        let bright = |y: &[f32]| level_at(y, hz(36.0) * 8.0) / level_at(y, hz(36.0)).max(1e-6);
        assert!(bright(&x[..4000]) > 3.0 * bright(&x[24_000..]));
    }

    #[test]
    fn single_triggering_glides_without_restarting_and_low_priority_holds_the_bass() {
        let mut s = synth();
        s.params.set(at::GLIDE, 0.1);
        s.params.set(at::PRIORITY, 0.0);
        s.params.set(at::ATTACK, 0.5);
        let x = render(
            &mut s,
            &[
                Event::note_on(0, 0, 36, 100, 0),
                Event::note_on(2400, 0, 48, 100, 1),
            ],
            4800,
        );
        // The second note did not restart the slow attack: no dip.
        let before = measure::peak(&x[2000..2400]);
        let after = measure::peak(&x[2400..2800]);
        assert!(after >= before * 0.8, "{before} {after}");
        // Low priority: the lower key is the one sounding.
        assert_eq!(s.voice.note, 36);
        assert_eq!(s.alloc.note_of(0), Some(36));
        // Let the low one go: the high one takes over, by glide.
        render(&mut s, &[Event::note_off(0, 0, 36, 0)], 16);
        assert_eq!(s.voice.note, 48);
        assert!(s.voice.pitch < 40.0);
        render(&mut s, &[], 48_000);
        assert!((s.voice.pitch - 48.0).abs() < 0.01);
    }

    #[test]
    fn oscillator_three_modulates_the_pitch_by_the_wheel() {
        let mut s = synth();
        s.params.set(at::VINTAGE, 0.0);
        s.params.set(at::OSC3_KEYS, 1.0);
        s.params.set(at::OSC3_WAVE, 0.0);
        s.params.set(at::OSC3_RANGE, 0.0);
        s.params.set(at::OSC3_TUNE, 5.2);
        s.params.set(at::OSC2_LEVEL, 0.0);
        s.params.set(at::OSC_MOD, 1.0);
        s.params.set(at::CUTOFF, 18_000.0);
        s.params.set(at::CONTOUR, 0.0);
        let still = render(&mut s, &[Event::note_on(0, 0, 69, 127, 0)], 48_000);
        s.event(&Event {
            frame: 0,
            channel: 0,
            kind: Kind::Control {
                number: 1,
                value: 127,
            },
        });
        let wobbly = render(&mut s, &[], 48_000);
        // A vibrato spreads the energy off the fundamental.
        assert!(level_at(&wobbly, 440.0) < 0.5 * level_at(&still, 440.0));
        assert!(measure::peak(&wobbly) > 0.05);
    }

    #[test]
    fn pink_noise_has_less_top_than_white() {
        let mut s = synth();
        s.params.set(at::OSC1_LEVEL, 0.0);
        s.params.set(at::OSC2_LEVEL, 0.0);
        s.params.set(at::NOISE_LEVEL, 1.0);
        s.params.set(at::CUTOFF, 18_000.0);
        s.params.set(at::CONTOUR, 0.0);
        s.params.set(at::OVERDRIVE, 0.0);
        let band = |x: &[f32], hz: f32| -> f32 {
            (0..8)
                .map(|k| level_at(x, hz * (1.0 + 0.03 * k as f32)))
                .sum::<f32>()
        };
        let white = render(&mut s, &[Event::note_on(0, 0, 60, 127, 0)], 96_000);
        s.params.set(at::NOISE_COLOUR, 1.0);
        let pink = render(&mut s, &[], 96_000);
        let tilt = |x: &[f32]| band(x, 100.0) / band(x, 6400.0);
        assert!(
            tilt(&pink) > 4.0 * tilt(&white),
            "{} {}",
            tilt(&pink),
            tilt(&white)
        );
    }

    /// A fixed patch renders to a known spectrum, so a later change to
    /// the toolkit can't silently alter the sound. The levels are those
    /// measured when the synth was built; a change that is meant to
    /// alter it updates them after listening.
    #[test]
    fn a_fixed_patch_renders_to_its_known_spectrum() {
        let mut s = synth();
        s.params.set(at::VINTAGE, 0.0);
        for (k, v) in sounds()[0].settings {
            s.params.set_key(k, *v);
        }
        s.params.set(at::OSC2_TUNE, 0.0);
        s.params.set(at::SUSTAIN, 1.0);
        let x = render(&mut s, &[Event::note_on(0, 0, 36, 100, 0)], 48_000);
        let f0 = hz(36.0) / 2.0;
        let got: Vec<f32> = (1..=8)
            .map(|h| 20.0 * level_at(&x, f0 * h as f32).max(1e-9).log10())
            .collect();
        let known = GOLDEN;
        for (h, (g, k)) in got.iter().zip(known).enumerate() {
            assert!(
                (g - k).abs() < 1.5,
                "harmonic {}: {g:.1} dB, was {k:.1}: {got:?}",
                h + 1
            );
        }
    }

    /// The fat bass at C2's harmonics of its 32' sub (32.7 Hz), 1 to 8,
    /// in dB, held after the contour has settled.
    const GOLDEN: [f32; 8] = [-17.3, -24.5, -27.8, -30.4, -33.4, -40.9, -48.0, -47.2];

    /// The plan's CPU budget, for one voice: a small fraction of one
    /// core at 48 kHz.
    #[test]
    #[cfg_attr(debug_assertions, ignore = "timed in release builds only")]
    fn one_voice_costs_little() {
        let mut s = synth();
        s.params.set(at::OVERDRIVE, 0.6);
        s.params.set(at::OSC3_LEVEL, 0.5);
        s.params.set(at::NOISE_LEVEL, 0.2);
        s.params.set(at::OSC_MOD, 1.0);
        s.params.set(at::MOD_AMOUNT, 0.3);
        let seconds = 2.0;
        let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
        let start = std::time::Instant::now();
        s.process(&[Event::note_on(0, 0, 36, 100, 0)], &mut l, &mut r);
        for _ in 0..(RATE * seconds / 512.0) as usize {
            s.process(&[], &mut l, &mut r);
        }
        let used = start.elapsed().as_secs_f64() / seconds as f64;
        assert!(used < 0.05, "{:.2}% of one core", used * 100.0);
    }
}
