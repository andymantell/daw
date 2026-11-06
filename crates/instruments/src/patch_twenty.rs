// SPDX-License-Identifier: GPL-3.0-or-later
//! Patch Twenty (classic synths plan, S7): a semi-modular monosynth in
//! the shape of the late-1970s Japanese one with two filters in series.
//! Two oscillators (triangle, saw, pulse or noise; saw, square, narrow
//! pulse or a ring modulator) into a 12 dB high-pass and then a 12 dB
//! low-pass, each a Sallen-Key pair whose resonance ("peak") path clips
//! so both sing and scream; a delay-attack-release envelope and a
//! hold-ADSR; a modulation generator whose shape sweeps from a falling
//! saw through a triangle to a rising one; an external signal input
//! (a guitar on the track's input) that runs through the filters and
//! can trigger the envelopes; and four patch cords, each a source, a
//! destination and an amount, in place of the panel's jacks.
//!
//! Built on the vintage toolkit ([`crate::vintage`]): the oscillators
//! drift, the filters saturate at twice the sample rate, the envelopes
//! have analogue curves and restart from where they are.

use daw_engine::midi::{Event, Instrument, Kind};

use crate::dsp::{Noise, db, hz};
use crate::params::{Live, Param, Unit};
use crate::vintage::{
    Action, Allocator, Analog, Curve, Drift, Glide, GlideMode, Osc, Priority, SallenKey, Shape,
    SkMode, Steps, VoiceSettings, Wave, soft,
};

const OSC1_WAVES: &[&str] = &["Triangle", "Saw", "Pulse", "Noise"];
const OSC1_OCTAVES: &[&str] = &["32'", "16'", "8'", "4'"];
const OSC2_WAVES: &[&str] = &["Saw", "Square", "Narrow pulse", "Ring mod"];
const OSC2_OCTAVES: &[&str] = &["16'", "8'", "4'", "2'"];
const VCA_SOURCES: &[&str] = &["Envelope 2", "Envelope 1", "Gate"];
/// What a patch cord can carry.
const SOURCES: &[&str] = &[
    "Off",
    "Mod generator triangle",
    "Mod generator square",
    "Envelope 1",
    "Envelope 2",
    "Sample and hold",
    "Noise",
    "Keyboard",
    "Velocity",
    "External follower",
];
/// Where a patch cord can go.
const DESTINATIONS: &[&str] = &[
    "Nothing",
    "Osc 1 pitch",
    "Osc 2 pitch",
    "Both pitches",
    "Pulse width",
    "High-pass cutoff",
    "Low-pass cutoff",
    "Both cutoffs",
    "Amplifier",
    "Mod generator rate",
];

/// How many patch cords there are.
pub const CORDS: usize = 4;
/// Settings a cord takes: source, destination, amount.
pub const CORD: usize = 3;

/// Settings by index; the order of [`params`].
pub mod at {
    pub const OSC1: usize = 0;
    pub const OSC1_OCTAVE: usize = 1;
    pub const WIDTH: usize = 2;
    pub const OSC1_LEVEL: usize = 3;
    pub const OSC2: usize = 4;
    pub const OSC2_OCTAVE: usize = 5;
    pub const OSC2_PITCH: usize = 6;
    pub const OSC2_LEVEL: usize = 7;
    pub const PITCH_MG: usize = 8;
    pub const PITCH_EG1: usize = 9;
    pub const GLIDE: usize = 10;
    pub const HP_CUTOFF: usize = 11;
    pub const HP_PEAK: usize = 12;
    pub const HP_MG: usize = 13;
    pub const HP_EG2: usize = 14;
    pub const LP_CUTOFF: usize = 15;
    pub const LP_PEAK: usize = 16;
    pub const LP_MG: usize = 17;
    pub const LP_EG2: usize = 18;
    pub const DRIVE: usize = 19;
    pub const MG_SHAPE: usize = 20;
    pub const MG_RATE: usize = 21;
    pub const EG1_DELAY: usize = 22;
    pub const EG1_ATTACK: usize = 23;
    pub const EG1_RELEASE: usize = 24;
    pub const EG2_HOLD: usize = 25;
    pub const EG2_ATTACK: usize = 26;
    pub const EG2_DECAY: usize = 27;
    pub const EG2_SUSTAIN: usize = 28;
    pub const EG2_RELEASE: usize = 29;
    pub const VCA_SOURCE: usize = 30;
    pub const DRONE: usize = 31;
    pub const EXT_IN: usize = 32;
    pub const EXT_GAIN: usize = 33;
    pub const EXT_TRIGGER: usize = 34;
    pub const EXT_THRESHOLD: usize = 35;
    pub const CORDS: usize = 36;
    pub const PRIORITY: usize = 48;
    pub const RETRIGGER: usize = 49;
    pub const VELOCITY: usize = 50;
    pub const BEND: usize = 51;
    pub const LEVEL: usize = 52;
    pub const VINTAGE: usize = 53;
}

/// The index of cord `c`'s setting `k` (0 source, 1 destination, 2
/// amount).
pub fn cord_at(c: usize, k: usize) -> usize {
    at::CORDS + c * CORD + k
}

/// The synth's settings, in the order of [`at`]. Grouped for showing by
/// [`GROUPS`].
pub fn params() -> Vec<Param> {
    use Unit::*;
    let mut v = vec![
        Param::new("osc1", "Osc 1 wave", 0.0, 3.0, 1.0, Choice).choices(OSC1_WAVES),
        Param::new("osc1_octave", "Osc 1 range", 0.0, 3.0, 2.0, Choice).choices(OSC1_OCTAVES),
        Param::new("width", "Pulse width", 0.05, 0.95, 0.5, Amount),
        Param::new("osc1_level", "Osc 1 level", 0.0, 1.0, 0.8, Amount),
        Param::new("osc2", "Osc 2 wave", 0.0, 3.0, 1.0, Choice).choices(OSC2_WAVES),
        Param::new("osc2_octave", "Osc 2 range", 0.0, 3.0, 1.0, Choice).choices(OSC2_OCTAVES),
        Param::new("osc2_pitch", "Osc 2 pitch", -12.0, 12.0, 0.0, Semitones),
        Param::new("osc2_level", "Osc 2 level", 0.0, 1.0, 0.5, Amount),
        Param::new(
            "pitch_mg",
            "Pitch from mod generator",
            0.0,
            12.0,
            0.0,
            Semitones,
        ),
        Param::new(
            "pitch_eg1",
            "Pitch from envelope 1",
            -12.0,
            12.0,
            0.0,
            Semitones,
        ),
        Param::new("glide", "Portamento", 0.0, 2.0, 0.0, Seconds),
        Param::new("hp_cutoff", "High-pass cutoff", 10.0, 18_000.0, 10.0, Hz).log(),
        Param::new("hp_peak", "High-pass peak", 0.0, 1.0, 0.0, Amount),
        Param::new(
            "hp_mg",
            "High-pass from mod generator",
            -4.0,
            4.0,
            0.0,
            Octaves,
        ),
        Param::new(
            "hp_eg2",
            "High-pass from envelope 2",
            -4.0,
            4.0,
            0.0,
            Octaves,
        ),
        Param::new("lp_cutoff", "Low-pass cutoff", 20.0, 18_000.0, 2_000.0, Hz).log(),
        Param::new("lp_peak", "Low-pass peak", 0.0, 1.0, 0.3, Amount),
        Param::new(
            "lp_mg",
            "Low-pass from mod generator",
            -4.0,
            4.0,
            0.0,
            Octaves,
        ),
        Param::new(
            "lp_eg2",
            "Low-pass from envelope 2",
            -4.0,
            4.0,
            2.0,
            Octaves,
        ),
        Param::new("drive", "Drive into the filters", 0.0, 1.0, 0.2, Amount),
        Param::new("mg_shape", "Mod generator shape", 0.0, 1.0, 0.5, Amount),
        Param::new("mg_rate", "Mod generator rate", 0.1, 30.0, 5.0, Hz).log(),
        Param::new("eg1_delay", "Envelope 1 delay", 0.0, 5.0, 0.0, Seconds),
        Param::new("eg1_attack", "Envelope 1 attack", 0.001, 5.0, 0.1, Seconds).log(),
        Param::new(
            "eg1_release",
            "Envelope 1 release",
            0.001,
            8.0,
            0.3,
            Seconds,
        )
        .log(),
        Param::new("eg2_hold", "Envelope 2 hold", 0.0, 5.0, 0.0, Seconds),
        Param::new(
            "eg2_attack",
            "Envelope 2 attack",
            0.001,
            5.0,
            0.005,
            Seconds,
        )
        .log(),
        Param::new("eg2_decay", "Envelope 2 decay", 0.01, 8.0, 0.4, Seconds).log(),
        Param::new("eg2_sustain", "Envelope 2 sustain", 0.0, 1.0, 0.6, Amount),
        Param::new("eg2_release", "Envelope 2 release", 0.01, 8.0, 0.3, Seconds).log(),
        Param::new("vca_source", "Amplifier follows", 0.0, 2.0, 0.0, Choice).choices(VCA_SOURCES),
        Param::new("drone", "Drone (initial gain)", 0.0, 1.0, 0.0, Amount),
        Param::new("ext_in", "External input", 0.0, 1.0, 0.0, Switch),
        Param::new("ext_gain", "External gain", 0.0, 40.0, 20.0, Db),
        Param::new(
            "ext_trigger",
            "External input triggers",
            0.0,
            1.0,
            0.0,
            Switch,
        ),
        Param::new("ext_threshold", "Trigger threshold", -60.0, 0.0, -30.0, Db),
    ];
    for c in 0..CORDS {
        let n = c + 1;
        v.push(
            Param::new(
                format!("patch{n}_from"),
                format!("Cord {n} from"),
                0.0,
                0.0,
                0.0,
                Choice,
            )
            .choices(SOURCES),
        );
        v.push(
            Param::new(
                format!("patch{n}_to"),
                format!("Cord {n} to"),
                0.0,
                0.0,
                0.0,
                Choice,
            )
            .choices(DESTINATIONS),
        );
        v.push(Param::new(
            format!("patch{n}_amount"),
            format!("Cord {n} amount"),
            -1.0,
            1.0,
            0.0,
            Amount,
        ));
    }
    v.extend([
        Param::new("priority", "Note priority", 0.0, 2.0, 2.0, Choice).choices(Priority::NAMES),
        Param::new("retrigger", "Retrigger envelopes", 0.0, 1.0, 1.0, Switch),
        Param::new("velocity", "Velocity", 0.0, 1.0, 0.5, Amount),
        Param::new("bend", "Bend range", 1.0, 24.0, 2.0, Semitones),
        Param::new("level", "Level", -36.0, 6.0, -8.0, Db),
        Param::new("vintage", "Vintage drift", 0.0, 1.0, 0.3, Amount),
    ]);
    debug_assert_eq!(v.len(), at::VINTAGE + 1);
    // The first cord, as the panel is normally used: the keyboard
    // opens the low-pass a little as the notes go up.
    v[cord_at(0, 0)].default = 7.0;
    v[cord_at(0, 1)].default = 6.0;
    v[cord_at(0, 2)].default = 0.5;
    v
}

/// The settings in groups, for the controls: (heading, first, last).
pub const GROUPS: &[(&str, usize, usize)] = &[
    ("Oscillators", at::OSC1, at::GLIDE),
    ("High-pass filter", at::HP_CUTOFF, at::HP_EG2),
    ("Low-pass filter", at::LP_CUTOFF, at::DRIVE),
    ("Mod generator", at::MG_SHAPE, at::MG_RATE),
    (
        "Envelope 1 (delay, attack, release)",
        at::EG1_DELAY,
        at::EG1_RELEASE,
    ),
    ("Envelope 2 (hold, ADSR)", at::EG2_HOLD, at::EG2_RELEASE),
    ("Amplifier", at::VCA_SOURCE, at::DRONE),
    ("External input", at::EXT_IN, at::EXT_THRESHOLD),
    ("Patch cords", at::CORDS, at::PRIORITY - 1),
    ("Playing", at::PRIORITY, at::VINTAGE),
];

/// Settings are read, the filters set and the modulations worked out
/// this often, in samples.
const CHUNK: usize = 16;

/// The delay-attack-release envelope: nothing for the delay, a climb to
/// full, full while the gate holds, then a fall.
#[derive(Clone, Copy)]
struct DelayEnv {
    env: Analog,
    /// Samples of delay still to wait before the attack, if waiting.
    delay: Option<f32>,
}

impl DelayEnv {
    const IDLE: DelayEnv = DelayEnv {
        env: Analog::IDLE,
        delay: None,
    };

    fn trigger(&mut self, delay_samples: f32) {
        if delay_samples > 0.0 {
            self.delay = Some(delay_samples);
        } else {
            self.delay = None;
            self.env.trigger();
        }
    }

    fn release(&mut self) {
        self.delay = None;
        self.env.release();
    }

    fn active(&self) -> bool {
        self.delay.is_some() || self.env.active()
    }

    #[inline]
    fn next(&mut self, s: &Steps) -> f32 {
        if let Some(d) = &mut self.delay {
            *d -= 1.0;
            if *d <= 0.0 {
                self.delay = None;
                self.env.trigger();
            } else {
                return self.env.level;
            }
        }
        self.env.next(s)
    }
}

/// The modulation generator: a wave whose rising part takes `shape` of
/// the cycle (0 a falling saw, 0.5 a triangle, 1 a rising saw) and a
/// square high for that part; a sample-and-hold of noise clocked by it.
#[derive(Clone, Copy, Default)]
struct ModGen {
    phase: f32,
    held: f32,
}

impl ModGen {
    /// Advances `samples` at `hz`, and returns (triangle, square, sample
    /// and hold), each −1 to 1.
    fn next(
        &mut self,
        hz: f32,
        shape: f32,
        samples: f32,
        rate: f32,
        noise: &mut Noise,
    ) -> (f32, f32, f32) {
        self.phase += hz * samples / rate;
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
            self.held = noise.tick();
        }
        let w = shape.clamp(0.02, 0.98);
        let p = self.phase;
        let tri = if p < w {
            -1.0 + 2.0 * p / w
        } else {
            1.0 - 2.0 * (p - w) / (1.0 - w)
        };
        let square = if p < w { 1.0 } else { -1.0 };
        (tri, square, self.held)
    }
}

/// What a patch cord is set to.
#[derive(Clone, Copy)]
struct Cord {
    from: u32,
    to: u32,
    amount: f32,
}

/// The modulations the cords add up to, per chunk.
#[derive(Clone, Copy, Default)]
struct Mods {
    /// Semitones.
    pitch1: f32,
    pitch2: f32,
    /// Added to the pulse width.
    width: f32,
    /// Octaves.
    hp: f32,
    lp: f32,
    /// Added to the amplifier's gain.
    amp: f32,
    /// Octaves on the mod generator's rate.
    mg_rate: f32,
}

/// The synth on the audio thread.
pub struct PatchTwenty {
    params: Live,
    rate: f32,
    alloc: Allocator,
    actions: Vec<Action>,
    osc1: Osc,
    osc2: Osc,
    drift: [Drift; 2],
    noise: Noise,
    hp: SallenKey,
    lp: SallenKey,
    mg: ModGen,
    eg1: DelayEnv,
    eg2: Analog,
    /// Samples of hold left after the key went up, if holding.
    hold: Option<f32>,
    /// The key is down (or the pedal holds it).
    key_gate: bool,
    /// The external input is over its threshold and triggering.
    ext_gate: bool,
    /// The external input's envelope follower, 0 to 1 of full scale
    /// after its gain.
    follower: f32,
    note: u8,
    /// The pitch now, gliding towards `note`.
    pitch: f32,
    velocity: f32,
    bend: f32,
    /// The gate's smoothed level, for an amplifier that follows it.
    gate_level: f32,
    /// The amplifier's offset (drone and cords), smoothed.
    offset: f32,
    mods: Mods,
    /// The last chunk's mod generator rate modulation, in octaves.
    mg_mod: f32,
    sounding: bool,
}

impl PatchTwenty {
    pub fn new(rate: u32, params: impl Into<Live>) -> PatchTwenty {
        let mut s = PatchTwenty {
            params: params.into(),
            rate: rate.max(1) as f32,
            alloc: Allocator::new(1),
            actions: Vec::with_capacity(8),
            osc1: Osc::new(Wave::Saw),
            osc2: Osc::new(Wave::Pulse),
            drift: [Drift::new(11), Drift::new(23)],
            noise: Noise::new(0x2020_1978),
            hp: SallenKey::new(),
            lp: SallenKey::new(),
            mg: ModGen::default(),
            eg1: DelayEnv::IDLE,
            eg2: Analog::IDLE,
            hold: None,
            key_gate: false,
            ext_gate: false,
            follower: 0.0,
            note: 48,
            pitch: 48.0,
            velocity: 1.0,
            bend: 0.0,
            gate_level: 0.0,
            offset: 0.0,
            mods: Mods::default(),
            mg_mod: 0.0,
            sounding: false,
        };
        s.osc1.reset(0.0);
        s.osc2.reset(0.37);
        s
    }

    fn voice_settings(&self) -> VoiceSettings {
        VoiceSettings {
            notes: 1,
            unison: 1,
            priority: Priority::from_index(self.params.read(at::PRIORITY)),
            legato: self.params.read(at::RETRIGGER) < 0.5,
        }
    }

    /// Starts both envelopes (from where they are).
    fn trigger(&mut self) {
        let delay = self.params.read(at::EG1_DELAY) * self.rate;
        self.eg1.trigger(delay);
        self.hold = None;
        self.eg2.trigger();
        self.sounding = true;
    }

    /// Lets both envelopes go, envelope 2 after its hold.
    fn release(&mut self) {
        self.eg1.release();
        let hold = self.params.read(at::EG2_HOLD) * self.rate;
        if hold > 0.0 {
            self.hold = Some(hold);
        } else {
            self.eg2.release();
        }
    }

    fn apply(&mut self) {
        let glide = self.params.read(at::GLIDE);
        let sens = self.params.read(at::VELOCITY);
        for i in 0..self.actions.len() {
            match self.actions[i] {
                Action::Start { note, velocity, .. } => {
                    if glide <= 0.0 || !self.sounding {
                        self.pitch = note as f32;
                    }
                    self.note = note;
                    self.velocity = 1.0 - sens + sens * velocity.clamp(1, 127) as f32 / 127.0;
                    self.key_gate = true;
                    self.trigger();
                }
                Action::Retune { note, .. } => {
                    self.note = note;
                    if glide <= 0.0 {
                        self.pitch = note as f32;
                    }
                }
                Action::Release { .. } => {
                    self.key_gate = false;
                    if !self.ext_gate {
                        self.release();
                    }
                }
            }
        }
        self.actions.clear();
    }

    fn event(&mut self, e: &Event) {
        let s = self.voice_settings();
        match e.kind {
            Kind::NoteOn {
                pitch, velocity, ..
            } => self.alloc.note_on(pitch, velocity, &s, &mut self.actions),
            Kind::NoteOff { pitch, .. } => self.alloc.note_off(pitch, &s, &mut self.actions),
            Kind::PitchBend(b) => self.bend = b as f32 / 8192.0,
            Kind::Control { number: 64, value } => self.alloc.pedal(value >= 64, &mut self.actions),
            Kind::Control { number: 120, .. } | Kind::AllNotesOff => {
                self.alloc.all_off(&mut self.actions);
            }
            _ => {}
        }
        self.apply();
    }

    /// The cords' modulations from this chunk's sources.
    fn cords(&self, sources: &[f32; 10]) -> Mods {
        let mut m = Mods::default();
        for c in 0..CORDS {
            let cord = Cord {
                from: self.params.read(cord_at(c, 0)) as u32,
                to: self.params.read(cord_at(c, 1)) as u32,
                amount: self.params.read(cord_at(c, 2)),
            };
            if cord.from == 0 || cord.to == 0 || cord.amount == 0.0 {
                continue;
            }
            let v = sources.get(cord.from as usize).copied().unwrap_or(0.0) * cord.amount;
            match cord.to {
                1 => m.pitch1 += 12.0 * v,
                2 => m.pitch2 += 12.0 * v,
                3 => {
                    m.pitch1 += 12.0 * v;
                    m.pitch2 += 12.0 * v;
                }
                4 => m.width += 0.45 * v,
                5 => m.hp += 4.0 * v,
                6 => m.lp += 4.0 * v,
                7 => {
                    m.hp += 4.0 * v;
                    m.lp += 4.0 * v;
                }
                8 => m.amp += v,
                9 => m.mg_rate += 2.0 * v,
                _ => {}
            }
        }
        m
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
        let r = self.rate;
        let n = (to - from) as f32;
        let ext_in = self.params.read(at::EXT_IN) >= 0.5;
        let ext_gain = db(self.params.read(at::EXT_GAIN));
        let ext_trigger = self.params.read(at::EXT_TRIGGER) >= 0.5;
        let threshold = db(self.params.read(at::EXT_THRESHOLD));
        // The external trigger: the follower over the threshold gates
        // the envelopes like a key, with some hysteresis.
        if ext_trigger && ext_in {
            let over = self.follower > threshold;
            let under = self.follower < threshold * 0.5;
            if over && !self.ext_gate {
                self.ext_gate = true;
                if !self.key_gate {
                    self.trigger();
                }
            } else if under && self.ext_gate {
                self.ext_gate = false;
                if !self.key_gate {
                    self.release();
                }
            }
        } else if self.ext_gate {
            self.ext_gate = false;
            if !self.key_gate {
                self.release();
            }
        }
        let p = &self.params;
        let drone = p.read(at::DRONE);
        let vca = p.read(at::VCA_SOURCE) as u32;
        let gate = self.key_gate || self.ext_gate;
        let awake = self.sounding || ext_in || drone > 0.0 || (vca == 2 && gate);
        if !awake {
            // Nothing to hear: keep the follower listening, then rest.
            if ext_trigger {
                for i in from..to {
                    let x = 0.5 * (left[i] + right[i]) * ext_gain;
                    follow(&mut self.follower, x, r);
                }
            }
            return;
        }
        // The mod generator, once a chunk.
        let mg_hz = p.read(at::MG_RATE) * self.mg_mod.exp2();
        let (mg_tri, mg_sq, sh) = self
            .mg
            .next(mg_hz, p.read(at::MG_SHAPE), n, r, &mut self.noise);
        let sources = [
            0.0,
            mg_tri,
            mg_sq,
            self.eg1.env.level,
            self.eg2.level,
            sh,
            self.noise.tick(),
            (self.pitch - 60.0) / 48.0,
            self.velocity,
            self.follower.min(1.0),
        ];
        let m = self.cords(&sources);
        self.mg_mod = m.mg_rate;
        self.mods = m;
        // Pitch: the note (gliding), bend, the panel's modulations, the
        // cords and each oscillator's own drift.
        let glide = Glide {
            mode: GlideMode::Time,
            seconds: p.read(at::GLIDE),
        };
        self.pitch = glide.step(self.pitch, self.note as f32, n, r);
        let vintage = p.read(at::VINTAGE);
        let common = self.pitch
            + self.bend * p.read(at::BEND)
            + p.read(at::PITCH_MG) * mg_tri
            + p.read(at::PITCH_EG1) * self.eg1.env.level;
        let oct1 = [-24.0, -12.0, 0.0, 12.0][(p.read(at::OSC1_OCTAVE) as usize).min(3)];
        let oct2 = [-12.0, 0.0, 12.0, 24.0][(p.read(at::OSC2_OCTAVE) as usize).min(3)];
        let d1 = self.drift[0].next(vintage, n, r) / 100.0;
        let d2 = self.drift[1].next(vintage, n, r) / 100.0;
        let pitch1 = common + oct1 + m.pitch1 + d1;
        let pitch2 = common + oct2 + p.read(at::OSC2_PITCH) + m.pitch2 + d2;
        let w1 = match p.read(at::OSC1) as u32 {
            0 => Some(Wave::Triangle),
            1 => Some(Wave::Saw),
            2 => Some(Wave::Pulse),
            _ => None,
        };
        let osc2_choice = p.read(at::OSC2) as u32;
        let (w2, width2) = match osc2_choice {
            0 => (Wave::Saw, 0.5),
            2 => (Wave::Pulse, 0.1),
            _ => (Wave::Pulse, 0.5),
        };
        if let Some(w) = w1
            && self.osc1.wave != w
        {
            self.osc1.wave = w;
            self.osc1.reset(self.osc1.phase);
        }
        if self.osc2.wave != w2 {
            self.osc2.wave = w2;
            self.osc2.reset(self.osc2.phase);
        }
        self.osc1.width = (p.read(at::WIDTH) + m.width).clamp(0.05, 0.95);
        self.osc2.width = width2;
        self.osc1.set_hz(hz(pitch1), r);
        self.osc2.set_hz(hz(pitch2), r);
        let level1 = p.read(at::OSC1_LEVEL);
        let level2 = p.read(at::OSC2_LEVEL);
        // The filters: cutoffs from the knobs, the panel's modulations
        // and the cords, in octaves; the drive is the gain into the
        // high-pass, whose clipping is the mixer's overdrive.
        let hp_oct = p.read(at::HP_MG) * mg_tri + p.read(at::HP_EG2) * self.eg2.level + m.hp;
        let lp_oct = p.read(at::LP_MG) * mg_tri + p.read(at::LP_EG2) * self.eg2.level + m.lp;
        let hp_fc = (p.read(at::HP_CUTOFF) * hp_oct.exp2()).clamp(5.0, 20_000.0);
        let lp_fc = (p.read(at::LP_CUTOFF) * lp_oct.exp2()).clamp(5.0, 20_000.0);
        let gain = 1.0 + 4.0 * p.read(at::DRIVE);
        self.hp
            .set(SkMode::High, hp_fc, p.read(at::HP_PEAK), gain, r);
        self.lp.set(SkMode::Low, lp_fc, p.read(at::LP_PEAK), 1.0, r);
        // The envelopes' steps.
        let eg1_steps = Steps::new(
            Shape {
                initial: 0.0,
                peak: 1.0,
                attack: p.read(at::EG1_ATTACK),
                decay: 1.0,
                sustain: 1.0,
                release: p.read(at::EG1_RELEASE),
                release_to: 0.0,
                curve: Curve::Analog,
            },
            r,
        );
        let eg2_steps = Steps::new(
            Shape::adsr(
                p.read(at::EG2_ATTACK),
                p.read(at::EG2_DECAY),
                p.read(at::EG2_SUSTAIN),
                p.read(at::EG2_RELEASE),
            ),
            r,
        );
        let offset_target = (drone + m.amp).clamp(0.0, 1.0);
        let out_gain = db(p.read(at::LEVEL)) * self.velocity;
        let gate_k = 1.0 - (-1.0 / (0.002 * r)).exp();
        let gate_target = if gate { 1.0 } else { 0.0 };
        let ring = osc2_choice == 3;
        for i in from..to {
            // The hold after the key goes up, then envelope 2's release.
            if let Some(h) = &mut self.hold {
                *h -= 1.0;
                if *h <= 0.0 {
                    self.hold = None;
                    self.eg2.release();
                }
            }
            let e1 = self.eg1.next(&eg1_steps);
            let e2 = self.eg2.next(&eg2_steps);
            self.gate_level += (gate_target - self.gate_level) * gate_k;
            self.offset += (offset_target - self.offset) * gate_k;
            let source = match vca {
                1 => e1,
                2 => self.gate_level,
                _ => e2,
            };
            let amp = (source + self.offset).clamp(0.0, 1.0);
            // The oscillators and the external signal into the mixer.
            let o1 = match w1 {
                Some(_) => self.osc1.tick(None),
                None => self.noise.tick(),
            };
            let o2raw = self.osc2.tick(None);
            let o2 = if ring { o1 * o2raw } else { o2raw };
            let mut mix = level1 * o1 + level2 * o2;
            if ext_in {
                let x = 0.5 * (left[i] + right[i]) * ext_gain;
                follow(&mut self.follower, x, r);
                mix += x;
            }
            // Through the two filters and the amplifier.
            let y = self.lp.run(self.hp.run(mix));
            let out = soft(y * amp * out_gain);
            if ext_in {
                left[i] = out;
                right[i] = out;
            } else {
                left[i] += out;
                right[i] += out;
            }
        }
        // Silent and nothing holding the amplifier open: rest.
        if !self.eg1.active() && !self.eg2.active() && self.hold.is_none() && !gate {
            self.sounding = false;
            if !ext_in && drone <= 0.0 && m.amp <= 0.0 {
                self.alloc.free(0);
            }
        }
    }
}

/// The envelope follower hears one sample of the external input: it
/// rises at once and falls over about 50 ms.
#[inline]
fn follow(follower: &mut f32, x: f32, rate: f32) {
    let a = x.abs();
    if a > *follower {
        *follower = a;
    } else {
        *follower *= 1.0 - 1.0 / (0.05 * rate);
    }
}

impl Instrument for PatchTwenty {
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

/// The sounds it makes from its settings alone, under the names they
/// came with (as presets first): a screaming lead, a bass, a drone and
/// a guitar through its filters.
pub fn sounds() -> Vec<crate::Sound> {
    vec![
        crate::Sound {
            name: "Patch Twenty screaming lead",
            settings: &[
                ("osc1", 2.0),
                ("osc2", 3.0),
                ("osc2_level", 0.7),
                ("hp_cutoff", 150.0),
                ("hp_peak", 0.7),
                ("lp_cutoff", 500.0),
                ("lp_peak", 0.95),
                ("lp_eg2", 3.0),
                ("drive", 0.8),
                ("eg2_decay", 0.5),
                ("eg2_sustain", 0.4),
                ("glide", 0.05),
                ("retrigger", 0.0),
                ("patch1_from", 7.0),
                ("patch1_to", 7.0),
                ("patch1_amount", 0.7),
                ("patch2_from", 1.0),
                ("patch2_to", 4.0),
                ("patch2_amount", 0.6),
                ("vintage", 0.5),
            ],
        },
        crate::Sound {
            name: "Patch Twenty bass",
            settings: &[
                ("osc1", 1.0),
                ("osc1_octave", 1.0),
                ("osc2", 1.0),
                ("osc2_octave", 0.0),
                ("osc2_level", 0.5),
                ("lp_cutoff", 250.0),
                ("lp_peak", 0.5),
                ("lp_eg2", 2.5),
                ("eg2_decay", 0.25),
                ("eg2_sustain", 0.1),
                ("drive", 0.5),
                ("glide", 0.06),
                ("retrigger", 0.0),
                ("vintage", 0.4),
            ],
        },
        crate::Sound {
            name: "Patch Twenty drone",
            settings: &[
                ("osc1", 1.0),
                ("osc1_octave", 1.0),
                ("osc2", 0.0),
                ("osc2_pitch", 7.0),
                ("osc2_level", 0.6),
                ("hp_cutoff", 120.0),
                ("hp_peak", 0.6),
                ("lp_cutoff", 400.0),
                ("lp_peak", 0.8),
                ("lp_eg2", 0.0),
                ("lp_mg", 1.5),
                ("mg_rate", 0.25),
                ("mg_shape", 0.3),
                ("drive", 0.6),
                ("drone", 1.0),
                ("patch1_from", 5.0),
                ("patch1_to", 5.0),
                ("patch1_amount", 0.8),
                ("patch2_from", 1.0),
                ("patch2_to", 9.0),
                ("patch2_amount", 0.5),
                ("vintage", 0.6),
            ],
        },
        crate::Sound {
            name: "Guitar through Patch Twenty",
            settings: &[
                ("osc1_level", 0.0),
                ("osc2_level", 0.0),
                ("ext_in", 1.0),
                ("ext_gain", 6.0),
                ("ext_trigger", 1.0),
                ("ext_threshold", -24.0),
                ("hp_cutoff", 90.0),
                ("hp_peak", 0.3),
                ("lp_cutoff", 300.0),
                ("lp_peak", 0.75),
                ("lp_eg2", 3.0),
                ("eg2_attack", 0.005),
                ("eg2_decay", 0.35),
                ("eg2_sustain", 0.2),
                ("eg2_release", 0.3),
                ("drive", 0.6),
                ("level", -4.0),
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::Params;
    use crate::vintage::measure;
    use std::sync::Arc;

    const RATE: u32 = 48_000;

    fn synth() -> PatchTwenty {
        PatchTwenty::new(RATE, Arc::new(Params::new(params())))
    }

    fn render(s: &mut PatchTwenty, events: &[Event], frames: usize) -> Vec<f32> {
        let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
        s.process(events, &mut l, &mut r);
        l
    }

    /// The level of `hz` in `x`'s second half, in dB.
    fn level_at(x: &[f32], hz: f32) -> f32 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        let half = x.len() / 2;
        for (i, &v) in x.iter().enumerate().skip(half) {
            let p = std::f32::consts::TAU * hz * i as f32 / RATE as f32;
            re += (v * p.cos()) as f64;
            im += (v * p.sin()) as f64;
        }
        let a = (re * re + im * im).sqrt() as f32 * 2.0 / (x.len() - half) as f32;
        20.0 * a.max(1e-9).log10()
    }

    #[test]
    fn the_settings_are_in_order_and_grouped() {
        let p = params();
        assert_eq!(p.len(), at::VINTAGE + 1);
        assert_eq!(p[at::LP_CUTOFF].key, "lp_cutoff");
        assert_eq!(p[cord_at(3, 2)].key, "patch4_amount");
        assert_eq!(p[at::PRIORITY].key, "priority");
        let mut next = 0;
        for &(_, from, to) in GROUPS {
            assert_eq!(from, next);
            assert!(to >= from);
            next = to + 1;
        }
        assert_eq!(next, p.len());
    }

    #[test]
    fn a_note_sounds_then_dies_away_and_the_voice_is_freed() {
        let mut s = synth();
        let x = render(&mut s, &[Event::note_on(100, 0, 48, 100, 0)], 4800);
        assert_eq!(measure::peak(&x[..100]), 0.0);
        assert!(
            measure::peak(&x[200..]) > 0.05,
            "{}",
            measure::peak(&x[200..])
        );
        let x = render(&mut s, &[Event::note_off(0, 0, 48, 0)], 96_000);
        assert_eq!(measure::peak(&x[90_000..]), 0.0);
        assert!(!s.sounding);
        assert_eq!(s.alloc.note_of(0), None);
    }

    #[test]
    fn its_pitch_follows_the_note_and_the_ranges() {
        let mut s = synth();
        s.params.set(at::OSC1, 0.0);
        s.params.set(at::OSC2_LEVEL, 0.0);
        s.params.set(at::LP_CUTOFF, 18_000.0);
        s.params.set(at::LP_EG2, 0.0);
        s.params.set(at::VINTAGE, 0.0);
        s.params.set(cord_at(0, 2), 0.0);
        let x = render(&mut s, &[Event::note_on(0, 0, 69, 127, 0)], 48_000);
        let f = measure::ring_hz(&x, RATE as f32);
        assert!((f - 440.0).abs() < 5.0, "{f}");
        // 16' is an octave down.
        s.params.set(at::OSC1_OCTAVE, 1.0);
        let x = render(&mut s, &[], 48_000);
        let f = measure::ring_hz(&x, RATE as f32);
        assert!((f - 220.0).abs() < 3.0, "{f}");
    }

    #[test]
    fn the_high_pass_thins_the_bass_and_the_low_pass_the_top() {
        let tone = |hp: f64, lp: f64| {
            let mut s = synth();
            s.params.set(at::OSC2_LEVEL, 0.0);
            s.params.set(at::LP_EG2, 0.0);
            s.params.set(at::DRIVE, 0.0);
            s.params.set(at::VINTAGE, 0.0);
            s.params.set(cord_at(0, 2), 0.0);
            s.params.set(at::HP_CUTOFF, hp);
            s.params.set(at::LP_CUTOFF, lp);
            render(&mut s, &[Event::note_on(0, 0, 45, 100, 0)], 48_000)
        };
        let f0 = hz(45.0);
        let open = tone(10.0, 18_000.0);
        let thin = tone(400.0, 18_000.0);
        let dull = tone(10.0, 300.0);
        assert!(level_at(&open, f0) - level_at(&thin, f0) > 15.0);
        assert!(level_at(&open, 8.0 * f0) - level_at(&dull, 8.0 * f0) > 15.0);
        // The top harmonic is about as strong either way with the
        // high-pass on, so the two filters are in series, not either-or.
        assert!((level_at(&open, 8.0 * f0) - level_at(&thin, 8.0 * f0)).abs() < 3.0);
    }

    #[test]
    fn both_filters_scream_on_their_own_with_the_peak_full() {
        for (which, fc) in [(at::HP_CUTOFF, 900.0), (at::LP_CUTOFF, 900.0)] {
            let mut s = synth();
            s.params.set(at::OSC1_LEVEL, 0.0);
            s.params.set(at::OSC2_LEVEL, 0.0);
            s.params.set(at::LP_EG2, 0.0);
            s.params.set(at::DRONE, 1.0);
            s.params.set(cord_at(0, 2), 0.0);
            s.params.set(which, fc);
            let peak = if which == at::HP_CUTOFF {
                at::HP_PEAK
            } else {
                at::LP_PEAK
            };
            s.params.set(peak, 1.0);
            if which == at::HP_CUTOFF {
                s.params.set(at::LP_CUTOFF, 18_000.0);
            }
            // A click to start it ringing: one sample of external input.
            s.params.set(at::EXT_IN, 1.0);
            let (mut l, mut r) = (vec![0.0; 48_000], vec![0.0; 48_000]);
            l[10] = 0.2;
            r[10] = 0.2;
            s.process(&[], &mut l, &mut r);
            let f = measure::ring_hz(&l, RATE as f32);
            assert!((f / fc as f32 - 1.0).abs() < 0.08, "{which}: {f}");
            assert!(measure::peak(&l[40_000..]) > 0.05, "{which}");
        }
    }

    #[test]
    fn the_external_input_goes_through_the_filters_and_replaces_the_dry_signal() {
        let mut s = synth();
        s.params.set(at::OSC1_LEVEL, 0.0);
        s.params.set(at::OSC2_LEVEL, 0.0);
        s.params.set(at::LP_EG2, 0.0);
        s.params.set(at::DRONE, 1.0);
        s.params.set(at::EXT_IN, 1.0);
        s.params.set(at::EXT_GAIN, 0.0);
        s.params.set(at::LP_CUTOFF, 200.0);
        s.params.set(at::LP_PEAK, 0.0);
        s.params.set(at::LEVEL, 0.0);
        let sine = |hz: f32, n: usize| -> Vec<f32> {
            (0..n)
                .map(|i| 0.3 * (std::f32::consts::TAU * hz * i as f32 / RATE as f32).sin())
                .collect()
        };
        let (mut l, mut r) = (sine(3000.0, 48_000), sine(3000.0, 48_000));
        s.process(&[], &mut l, &mut r);
        // 3 kHz through a 200 Hz low-pass: well down from the 0.3 it
        // went in at (−10.5 dB), so the dry signal is gone.
        assert!(level_at(&l, 3000.0) < -40.0, "{}", level_at(&l, 3000.0));
        let (mut l, mut r) = (sine(100.0, 48_000), sine(100.0, 48_000));
        s.process(&[], &mut l, &mut r);
        assert!(level_at(&l, 100.0) > -14.0, "{}", level_at(&l, 100.0));
        // Off (and nothing holding the amplifier open), the track's
        // audio is left alone and the synth is silent.
        s.params.set(at::EXT_IN, 0.0);
        s.params.set(at::DRONE, 0.0);
        let (mut l, mut r) = (sine(3000.0, 4800), sine(3000.0, 4800));
        let before = l.clone();
        s.process(&[], &mut l, &mut r);
        assert_eq!(l, before);
    }

    #[test]
    fn the_external_input_triggers_the_envelopes_without_a_key() {
        let mut s = synth();
        s.params.set(at::EXT_IN, 1.0);
        s.params.set(at::EXT_GAIN, 0.0);
        s.params.set(at::EXT_TRIGGER, 1.0);
        s.params.set(at::EXT_THRESHOLD, -20.0);
        s.params.set(at::EG2_RELEASE, 0.05);
        s.params.set(at::OSC1_LEVEL, 0.8);
        // Quiet input: nothing.
        let (mut l, mut r) = (vec![0.01; 4800], vec![0.01; 4800]);
        s.process(&[], &mut l, &mut r);
        assert!(!s.ext_gate);
        assert!(!s.eg2.active());
        // A loud burst: the envelopes start, the oscillators sound,
        // and after it the envelopes release.
        let burst = |i: usize| 0.5 * ((i as f32) * 0.3).sin();
        let (mut l, mut r): (Vec<f32>, Vec<f32>) = (
            (0..4800).map(burst).collect(),
            (0..4800).map(burst).collect(),
        );
        s.process(&[], &mut l, &mut r);
        assert!(s.ext_gate);
        assert!(s.eg2.active());
        let (mut l, mut r) = (vec![0.0; 48_000], vec![0.0; 48_000]);
        s.process(&[], &mut l, &mut r);
        assert!(!s.ext_gate);
        assert!(!s.eg2.active());
    }

    #[test]
    fn a_patch_cord_from_the_keyboard_opens_the_low_pass() {
        let bright = |amount: f64| {
            let mut s = synth();
            s.params.set(at::OSC2_LEVEL, 0.0);
            s.params.set(at::LP_EG2, 0.0);
            s.params.set(at::LP_CUTOFF, 300.0);
            s.params.set(at::VINTAGE, 0.0);
            s.params.set(cord_at(0, 0), 7.0);
            s.params.set(cord_at(0, 1), 6.0);
            s.params.set(cord_at(0, 2), amount);
            let x = render(&mut s, &[Event::note_on(0, 0, 84, 100, 0)], 48_000);
            level_at(&x, 4.0 * hz(84.0))
        };
        // Two octaves above middle C with a full cord is two octaves
        // more cutoff, so the fourth harmonic comes up.
        assert!(
            bright(1.0) - bright(0.0) > 10.0,
            "{} {}",
            bright(0.0),
            bright(1.0)
        );
        // A cord to the amplifier from a source that is off leaves the
        // amplifier alone.
        let mut s = synth();
        s.params.set(cord_at(1, 0), 0.0);
        s.params.set(cord_at(1, 1), 8.0);
        s.params.set(cord_at(1, 2), 1.0);
        let x = render(&mut s, &[], 4800);
        assert_eq!(measure::peak(&x), 0.0);
    }

    #[test]
    fn envelope_one_waits_its_delay_and_envelope_two_holds() {
        let mut s = synth();
        s.params.set(at::VCA_SOURCE, 1.0);
        s.params.set(at::EG1_DELAY, 0.1);
        s.params.set(at::EG1_ATTACK, 0.001);
        let x = render(&mut s, &[Event::note_on(0, 0, 48, 100, 0)], 9600);
        assert_eq!(measure::peak(&x[..4000]), 0.0);
        assert!(measure::peak(&x[6000..]) > 0.05);
        let mut s = synth();
        s.params.set(at::EG2_HOLD, 0.2);
        s.params.set(at::EG2_SUSTAIN, 1.0);
        s.params.set(at::EG2_RELEASE, 0.01);
        render(&mut s, &[Event::note_on(0, 0, 48, 100, 0)], 4800);
        let x = render(&mut s, &[Event::note_off(0, 0, 48, 0)], 19_200);
        // Still full for the hold, then gone.
        assert!(measure::peak(&x[4800..7200]) > 0.1);
        assert_eq!(measure::peak(&x[14_400..]), 0.0);
    }

    #[test]
    fn mono_goes_back_to_the_held_note_and_legato_keeps_the_envelope() {
        let mut s = synth();
        s.params.set(at::RETRIGGER, 0.0);
        s.params.set(at::EG2_ATTACK, 1.0);
        render(&mut s, &[Event::note_on(0, 0, 48, 100, 0)], 4800);
        let before = s.eg2.level;
        render(
            &mut s,
            &[
                Event::note_on(0, 0, 55, 100, 1),
                Event::note_off(10, 0, 55, 1),
            ],
            64,
        );
        assert_eq!(s.note, 48);
        assert!(s.eg2.level >= before);
        s.params.set(at::PRIORITY, 0.0);
        render(
            &mut s,
            &[
                Event::note_on(0, 0, 40, 100, 0),
                Event::note_on(1, 0, 60, 100, 0),
            ],
            64,
        );
        assert_eq!(s.note, 40);
    }

    #[test]
    fn the_mod_generator_sweeps_from_saw_to_saw_and_wobbles_the_pitch() {
        let mut mg = ModGen::default();
        let mut noise = Noise::new(1);
        let shape = |s: f32| {
            let mut mg = ModGen::default();
            let mut noise = Noise::new(1);
            (0..100)
                .map(|_| mg.next(10.0, s, 1.0, 1000.0, &mut noise).0)
                .collect::<Vec<f32>>()
        };
        let down = shape(0.0);
        let up = shape(1.0);
        let tri = shape(0.5);
        // A falling saw is lower at the quarter than at the start; a
        // rising one is higher; a triangle peaks half way.
        assert!(down[3] > down[50], "{} {}", down[3], down[50]);
        assert!(up[50] > up[3], "{} {}", up[3], up[50]);
        assert!(tri[50] > 0.9 && tri[25] < 0.1, "{} {}", tri[50], tri[25]);
        let (_, sq, sh) = mg.next(10.0, 0.5, 1.0, 1000.0, &mut noise);
        assert_eq!(sq, 1.0);
        assert_eq!(sh, 0.0);
        let mut s = synth();
        s.params.set(at::PITCH_MG, 12.0);
        s.params.set(at::MG_RATE, 1.0);
        s.params.set(at::OSC1, 0.0);
        s.params.set(at::OSC2_LEVEL, 0.0);
        s.params.set(at::LP_CUTOFF, 18_000.0);
        let x = render(&mut s, &[Event::note_on(0, 0, 60, 100, 0)], 48_000);
        let a = measure::ring_hz(&x[12_000..24_000], RATE as f32);
        let b = measure::ring_hz(&x[36_000..48_000], RATE as f32);
        assert!((a / b - 1.0).abs() > 0.3, "{a} {b}");
    }

    /// The plan's golden render: a fixed patch has a known spectrum, so
    /// a later change to the toolkit can't alter it unnoticed. When a
    /// change is meant to alter the sound, listen, then put the new
    /// figures in.
    #[test]
    fn the_fixed_patch_renders_the_known_spectrum() {
        let mut s = synth();
        s.params.set(at::VINTAGE, 0.0);
        s.params.set(at::OSC2, 0.0);
        s.params.set(at::OSC2_LEVEL, 0.6);
        s.params.set(at::OSC2_PITCH, 7.0);
        s.params.set(at::HP_CUTOFF, 120.0);
        s.params.set(at::HP_PEAK, 0.4);
        s.params.set(at::LP_CUTOFF, 900.0);
        s.params.set(at::LP_PEAK, 0.6);
        s.params.set(at::DRIVE, 0.5);
        s.params.set(at::LP_EG2, 1.0);
        s.params.set(at::EG2_SUSTAIN, 1.0);
        let x = render(&mut s, &[Event::note_on(0, 0, 40, 100, 0)], 48_000);
        let f0 = hz(40.0);
        let got: Vec<f32> = [1.0, 2.0, 3.0, 4.0, 6.0, 8.0]
            .iter()
            .map(|h| level_at(&x, h * f0))
            .collect();
        let known = GOLDEN;
        for (h, (g, k)) in got.iter().zip(known).enumerate() {
            assert!(
                (g - k).abs() < 1.0,
                "harmonic {h}: {g:.2} dB, known {k:.2}; all {got:?}"
            );
        }
    }

    /// The fixed patch's harmonic levels in dB (see the golden test).
    const GOLDEN: [f32; 6] = [-19.19, -21.02, -38.19, -25.27, -25.33, -34.66];

    /// The plan's CPU budget, for one voice through two oversampled
    /// filters: well under a percent of one core at 48 kHz. The bound
    /// is loose because test machines differ; the measured figure is
    /// in the review notes.
    #[test]
    #[cfg_attr(debug_assertions, ignore = "timed in release builds only")]
    fn one_voice_fits_the_budget() {
        let mut s = synth();
        s.params.set(at::EG2_SUSTAIN, 1.0);
        let seconds = 2.0;
        let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
        let start = std::time::Instant::now();
        s.process(&[Event::note_on(0, 0, 36, 100, 0)], &mut l, &mut r);
        for _ in 0..(48_000.0 * seconds / 512.0) as usize {
            s.process(&[], &mut l, &mut r);
        }
        let used = start.elapsed().as_secs_f64() / seconds;
        eprintln!("one voice: {:.3}% of one core", used * 100.0);
        assert!(used < 0.05, "{:.2}% of one core", used * 100.0);
    }
}
