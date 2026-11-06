// SPDX-License-Identifier: GPL-3.0-or-later
//! Acid Three: a bass line synth built on the architecture of the
//! 1982 acid bass machine (`docs/plans/classic-synths.md`, S2). One
//! oscillator, saw or square, into the vintage toolkit's four-pole
//! diode ladder; a filter envelope with an attack and a decay only, an
//! amplifier envelope with a fixed decay, and the two things that make
//! its lines move:
//!
//! - **Accent.** A note played harder than the accent velocity is
//!   accented: it is louder, its filter envelope decays quickly however
//!   the decay knob is set, and an extra sweep is added to the cutoff
//!   that piles up over consecutive accents (the sweep's capacitor
//!   doesn't empty between them), rising with the resonance, as the
//!   circuit's does. A note without an accent lets the pile go down.
//! - **Slide.** A note that starts while the one before it is still
//!   held slides into it: the pitch glides over the slide time and the
//!   envelopes keep going instead of starting again.
//!
//! Accent is velocity and slide is overlap, so note clips and patterns
//! drive it with nothing more than notes. The behaviour follows the
//! published analysis of the circuit (Kernmayer, "The Sound of Acid",
//! 2020) and the plan's sources, written from scratch.

use daw_engine::midi::{Event, Instrument, Kind};

use crate::dsp::{db, hz};
use crate::params::{Live, Param, Unit};
use crate::vintage::{
    Action, Allocator, Analog, Diode, Drift, Drive, Glide, GlideMode, Osc, Priority, Shape, Steps,
    VoiceSettings, Wave,
};

const WAVES: &[&str] = &["Saw", "Square"];

/// Settings by index; the order of [`params`].
pub mod at {
    pub const WAVE: usize = 0;
    pub const TUNE: usize = 1;
    pub const CUTOFF: usize = 2;
    pub const RESONANCE: usize = 3;
    pub const ENV_MOD: usize = 4;
    pub const DECAY: usize = 5;
    pub const ACCENT: usize = 6;
    pub const ACCENT_VELOCITY: usize = 7;
    pub const SLIDE: usize = 8;
    pub const DRIVE: usize = 9;
    pub const VINTAGE: usize = 10;
    pub const BEND: usize = 11;
    pub const LEVEL: usize = 12;
}

/// The synth's settings, in the order of [`at`]. Grouped for showing by
/// [`GROUPS`].
pub fn params() -> Vec<Param> {
    use Unit::*;
    vec![
        Param::new("wave", "Waveform", 0.0, 1.0, 0.0, Choice).choices(WAVES),
        Param::new("tune", "Tuning", -12.0, 12.0, 0.0, Semitones),
        Param::new("cutoff", "Cutoff", 80.0, 8000.0, 500.0, Hz).log(),
        Param::new("resonance", "Resonance", 0.0, 1.0, 0.6, Amount),
        Param::new("env_mod", "Envelope amount", 0.0, 1.0, 0.5, Amount),
        Param::new("decay", "Decay", 0.05, 3.0, 0.4, Seconds).log(),
        Param::new("accent", "Accent", 0.0, 1.0, 0.6, Amount),
        Param::new(
            "accent_velocity",
            "Accent from velocity",
            1.0,
            127.0,
            100.0,
            Count,
        ),
        Param::new("slide", "Slide time", 0.01, 0.5, 0.06, Seconds).log(),
        Param::new("drive", "Drive", 0.0, 1.0, 0.0, Amount),
        Param::new("vintage", "Vintage drift", 0.0, 1.0, 0.3, Amount),
        Param::new("bend", "Bend range", 1.0, 24.0, 2.0, Semitones),
        Param::new("level", "Level", -36.0, 6.0, -3.0, Db),
    ]
}

/// The settings in groups, for the controls: (heading, first, last).
pub const GROUPS: &[(&str, usize, usize)] = &[
    ("Oscillator", at::WAVE, at::TUNE),
    ("Filter", at::CUTOFF, at::DECAY),
    ("Accent and slide", at::ACCENT, at::SLIDE),
    ("Character", at::DRIVE, at::LEVEL),
];

/// Sounds made here, as settings over the defaults, so the panel's
/// header and the assistant can step through them.
pub fn sounds() -> Vec<crate::Sound> {
    use crate::Sound;
    vec![
        Sound {
            name: "Squelch line",
            settings: &[
                ("cutoff", 350.0),
                ("resonance", 0.8),
                ("env_mod", 0.7),
                ("decay", 0.35),
                ("accent", 0.7),
            ],
        },
        Sound {
            name: "Deep round bass",
            settings: &[
                ("wave", 1.0),
                ("cutoff", 260.0),
                ("resonance", 0.25),
                ("env_mod", 0.3),
                ("decay", 0.6),
                ("accent", 0.4),
            ],
        },
        Sound {
            name: "Screaming lead",
            settings: &[
                ("cutoff", 900.0),
                ("resonance", 0.9),
                ("env_mod", 0.8),
                ("decay", 0.25),
                ("accent", 0.9),
                ("drive", 0.6),
                ("level", -6.0),
            ],
        },
        Sound {
            name: "Soft pluck",
            settings: &[
                ("wave", 1.0),
                ("cutoff", 600.0),
                ("resonance", 0.4),
                ("env_mod", 0.55),
                ("decay", 0.15),
                ("accent", 0.3),
                ("slide", 0.03),
            ],
        },
        Sound {
            name: "Rubber bass",
            settings: &[
                ("wave", 1.0),
                ("cutoff", 180.0),
                ("resonance", 0.7),
                ("env_mod", 0.6),
                ("decay", 0.3),
                ("accent", 0.5),
                ("slide", 0.1),
                ("drive", 0.2),
            ],
        },
        Sound {
            name: "Distorted acid",
            settings: &[
                ("cutoff", 450.0),
                ("resonance", 0.85),
                ("env_mod", 0.75),
                ("decay", 0.5),
                ("accent", 0.8),
                ("drive", 0.9),
                ("level", 0.0),
            ],
        },
        Sound {
            name: "Bleep",
            settings: &[
                ("wave", 1.0),
                ("tune", 12.0),
                ("cutoff", 1500.0),
                ("resonance", 0.6),
                ("env_mod", 0.4),
                ("decay", 0.08),
                ("accent", 0.5),
                ("slide", 0.02),
                ("level", -6.0),
            ],
        },
        Sound {
            name: "Slow open line",
            settings: &[
                ("cutoff", 220.0),
                ("resonance", 0.75),
                ("env_mod", 0.9),
                ("decay", 1.6),
                ("accent", 0.6),
                ("slide", 0.12),
            ],
        },
    ]
}

/// Settings are read, and the filter set, this often.
const CHUNK: usize = 16;
/// The filter envelope's attack.
const F_ATTACK: f32 = 0.003;
/// An accented note's filter decay, whatever the knob says.
const ACCENT_DECAY: f32 = 0.2;
/// The amplifier envelope: a fast attack, a long fixed decay, and a
/// short release when the gate ends.
const A_ATTACK: f32 = 0.003;
const A_DECAY: f32 = 2.5;
const A_RELEASE: f32 = 0.012;
/// How far the filter envelope opens the cutoff at full amount.
const ENV_OCTAVES: f32 = 4.5;
/// How far a full accent pile opens it on top.
const SWEEP_OCTAVES: f32 = 1.5;
/// The pile's most, in accents' worth.
const PILE_MAX: f32 = 2.0;
/// The pile's time constant as it drains, in seconds: slower than an
/// accented note's own envelope, which is what gives the "wow".
const PILE_DRAIN: f32 = 0.25;
/// The envelopes' times are where they are 99.9% done; the decay knob
/// reads better as where the sweep is mostly over, so it is stretched.
const DECAY_STRETCH: f32 = 3.0;
/// How quickly the accent sweep follows the pile, in seconds.
const SWEEP_RISE: f32 = 0.02;
/// The oscillator's level into the ladder: enough that its stages
/// warm the sound, not enough to square it off.
const OSC_LEVEL: f32 = 0.3;
const LADDER_DRIVE: f32 = 1.0;
/// The high-pass in the ladder's feedback and how much of the bass the
/// feedback takes is put back at the input (the rest after).
const FEEDBACK_HP: f32 = 60.0;
const COMPENSATION: f32 = 0.3;
/// The ladder loses level as its resonance rises (its feedback takes
/// it); this much is given back after it, per unit of feedback, so a
/// line stays about as loud whatever the resonance.
const MAKEUP_PER_K: f32 = 0.1;
/// The feedback the ladder runs at full resonance (its own constant).
const FULL_FEEDBACK: f32 = 22.0;

/// The synth on the audio thread.
pub struct Acid {
    params: Live,
    rate: f32,
    alloc: Allocator,
    actions: Vec<Action>,
    osc: Osc,
    filter: Diode,
    amp: Analog,
    fenv: Analog,
    drift: Drift,
    /// Whether the voice is sounding at all.
    on: bool,
    /// The pitch now and the note it is sliding to, in semitones.
    pitch: f32,
    target: f32,
    /// The note sounding is accented.
    accent: bool,
    /// Accents piled up in the sweep circuit, and the sweep that follows.
    pile: f32,
    sweep: f32,
    bend: f32,
}

impl Acid {
    pub fn new(rate: u32, params: impl Into<Live>) -> Acid {
        Acid {
            params: params.into(),
            rate: rate.max(1) as f32,
            alloc: Allocator::new(1),
            actions: Vec::with_capacity(4),
            osc: Osc::new(Wave::Saw),
            filter: Diode::new(),
            amp: Analog::IDLE,
            fenv: Analog::IDLE,
            drift: Drift::new(3),
            on: false,
            pitch: 36.0,
            target: 36.0,
            accent: false,
            pile: 0.0,
            sweep: 0.0,
            bend: 0.0,
        }
    }

    fn settings(&self) -> VoiceSettings {
        VoiceSettings {
            notes: 1,
            unison: 1,
            priority: Priority::Last,
            legato: true,
        }
    }

    fn apply(&mut self) {
        let actions = std::mem::take(&mut self.actions);
        for a in &actions {
            match *a {
                Action::Start {
                    note,
                    velocity,
                    fresh,
                    ..
                } => {
                    let threshold = self.params.read(at::ACCENT_VELOCITY);
                    self.accent = velocity as f32 >= threshold;
                    if fresh {
                        self.osc.reset(0.0);
                        self.filter.reset();
                        self.sweep = 0.0;
                    }
                    self.pitch = note as f32;
                    self.target = self.pitch;
                    self.amp.trigger();
                    self.fenv.trigger();
                    if self.accent {
                        let res = self.params.read(at::RESONANCE);
                        let amount = self.params.read(at::ACCENT);
                        self.pile = (self.pile + amount * (0.5 + 0.5 * res)).min(PILE_MAX);
                    } else {
                        // A plain note lets the sweep circuit empty.
                        self.pile *= 0.5;
                    }
                    self.on = true;
                }
                Action::Retune { note, .. } => {
                    // A slide: the pitch glides, nothing restarts.
                    self.target = note as f32;
                }
                Action::Release { .. } => {
                    self.amp.release();
                }
            }
        }
        self.actions = actions;
        self.actions.clear();
    }

    fn event(&mut self, e: &Event) {
        let s = self.settings();
        match e.kind {
            Kind::NoteOn {
                pitch, velocity, ..
            } => self.alloc.note_on(pitch, velocity, &s, &mut self.actions),
            Kind::NoteOff { pitch, .. } => self.alloc.note_off(pitch, &s, &mut self.actions),
            Kind::PitchBend(b) => self.bend = b as f32 / 8192.0,
            Kind::Control { number: 64, value } => {
                self.alloc.pedal(value >= 64, &mut self.actions);
            }
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
        if !self.on {
            return;
        }
        let p = &self.params;
        let r = self.rate;
        let n = (to - from) as f32;
        let wave = if p.read(at::WAVE) >= 0.5 {
            Wave::Pulse
        } else {
            Wave::Saw
        };
        let tune = p.read(at::TUNE);
        let cutoff = p.read(at::CUTOFF);
        let res = p.read(at::RESONANCE);
        let env_mod = p.read(at::ENV_MOD);
        let decay = if self.accent {
            ACCENT_DECAY
        } else {
            p.read(at::DECAY)
        };
        let accent = p.read(at::ACCENT);
        let glide = Glide {
            mode: GlideMode::Time,
            seconds: p.read(at::SLIDE),
        };
        let drive = Drive::new(p.read(at::DRIVE));
        let makeup = 1.0 + MAKEUP_PER_K * FULL_FEEDBACK * res;
        let vintage = p.read(at::VINTAGE);
        let bend = self.bend * p.read(at::BEND);
        let gain = db(p.read(at::LEVEL));
        let amp_steps = Steps::new(Shape::adsr(A_ATTACK, A_DECAY, 0.0, A_RELEASE), r);
        let decay = decay * DECAY_STRETCH;
        let f_steps = Steps::new(Shape::adsr(F_ATTACK, decay, 0.0, decay), r);

        // The pitch: sliding, bent, drifting.
        self.pitch = glide.step(self.pitch, self.target, n, r);
        let cents = self.drift.next(vintage, n, r);
        let pitch = self.pitch + tune + bend + cents / 100.0;
        self.osc.wave = wave;
        self.osc.width = 0.5;
        self.osc.set_hz(hz(pitch), r);

        // The accent sweep follows the pile, which drains on its own,
        // more slowly than the note's envelope.
        self.pile *= (-n / (PILE_DRAIN * r)).exp();
        let k = 1.0 - (-n / (SWEEP_RISE * r)).exp();
        self.sweep += (self.pile - self.sweep) * k;
        let octaves = ENV_OCTAVES * env_mod * self.fenv.level + SWEEP_OCTAVES * self.sweep;
        let fc = (cutoff * octaves.exp2()).min(r * 0.45);
        self.filter
            .set(fc, res, LADDER_DRIVE, FEEDBACK_HP, COMPENSATION, r);

        // An accented note is louder, and more so as the sweep piles up.
        let loud = if self.accent {
            1.0 + accent * (0.8 + 0.5 * self.sweep.min(1.0))
        } else {
            1.0
        };
        for i in from..to {
            let a = self.amp.next(&amp_steps);
            self.fenv.next(&f_steps);
            if !self.amp.active() {
                self.on = false;
                self.alloc.free(0);
                break;
            }
            let x = self.osc.tick(None) * OSC_LEVEL;
            let y = drive.run(self.filter.run(x) * makeup * a * loud) * gain;
            left[i] += y;
            right[i] += y;
        }
    }
}

impl Instrument for Acid {
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

    const RATE: usize = 48_000;

    fn acid() -> Acid {
        Acid::new(RATE as u32, Arc::new(Params::new(params())))
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    /// Renders `events` over `seconds`, one block.
    fn render(s: &mut Acid, events: &[Event], seconds: f32) -> Vec<f32> {
        let n = (RATE as f32 * seconds) as usize;
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        s.process(events, &mut l, &mut r);
        l
    }

    /// The amplitude at `hz` over `x`.
    fn level_at(x: &[f32], hz: f32) -> f32 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, &v) in x.iter().enumerate() {
            let p = std::f64::consts::TAU * hz as f64 * i as f64 / RATE as f64;
            re += v as f64 * p.cos();
            im += v as f64 * p.sin();
        }
        ((re * re + im * im).sqrt() * 2.0 / x.len() as f64) as f32
    }

    /// Where the spectrum's weight sits: the centroid in Hz of the
    /// harmonics of `f0` over `x`.
    fn centroid(x: &[f32], f0: f32) -> f32 {
        let (mut num, mut den) = (0.0f32, 0.0f32);
        for k in 1..=80 {
            let f = f0 * k as f32;
            let a = level_at(x, f);
            num += a * f;
            den += a;
        }
        num / den.max(1e-9)
    }

    #[test]
    fn a_note_sounds_at_its_pitch_and_stops_when_let_go() {
        let mut s = acid();
        s.params.set(at::VINTAGE, 0.0);
        s.params.set(at::CUTOFF, 8000.0);
        let out = render(&mut s, &[Event::note_on(0, 0, 45, 80, 0)], 1.0);
        assert!(peak(&out) > 0.05);
        let tail = &out[RATE / 2..];
        let f0 = hz(45.0);
        assert!(level_at(tail, f0) > level_at(tail, f0 * 1.5) * 3.0);
        let out = render(&mut s, &[Event::note_off(0, 0, 45, 0)], 0.5);
        assert_eq!(peak(&out[RATE / 4..]), 0.0);
        assert!(!s.on);
    }

    #[test]
    fn the_filter_envelope_opens_and_closes_the_sound() {
        let mut s = acid();
        s.params.set(at::DECAY, 0.3);
        s.params.set(at::ENV_MOD, 1.0);
        let out = render(&mut s, &[Event::note_on(0, 0, 36, 80, 0)], 1.0);
        let f0 = hz(36.0);
        let early = centroid(&out[..RATE / 20], f0);
        let late = centroid(&out[RATE / 2..RATE * 3 / 5], f0);
        assert!(early > late * 2.0, "{early} {late}");
    }

    #[test]
    fn an_accent_is_louder_and_brighter_and_piles_up() {
        let line = |velocities: &[u8]| {
            let mut s = acid();
            s.params.set(at::VINTAGE, 0.0);
            s.params.set(at::ENV_MOD, 0.3);
            s.params.set(at::DECAY, 0.2);
            let mut out = Vec::new();
            for &v in velocities {
                let on = render(&mut s, &[Event::note_on(0, 0, 36, v, 0)], 0.09);
                let off = render(&mut s, &[Event::note_off(0, 0, 36, 0)], 0.035);
                out.extend(on);
                out.extend(off);
            }
            out
        };
        let plain = line(&[80]);
        let accented = line(&[120]);
        let f0 = hz(36.0);
        assert!(peak(&accented) > peak(&plain) * 1.15);
        assert!(centroid(&accented, f0) > centroid(&plain, f0) * 1.2);
        // Four accents in a row sweep higher than one.
        let four = line(&[120, 120, 120, 120]);
        let n = plain.len();
        let last = &four[3 * n..3 * n + n / 2];
        let first = &accented[..n / 2];
        assert!(centroid(last, f0) > centroid(first, f0) * 1.1);
    }

    #[test]
    fn overlapping_notes_slide_without_restarting() {
        let mut s = acid();
        s.params.set(at::VINTAGE, 0.0);
        s.params.set(at::SLIDE, 0.1);
        render(&mut s, &[Event::note_on(0, 0, 36, 80, 0)], 0.3);
        let before = s.amp.level;
        // The next note starts while the first is held: a slide.
        render(&mut s, &[Event::note_on(0, 0, 48, 80, 0)], 0.02);
        assert!(s.amp.level < before, "the envelope restarted");
        assert!(s.pitch > 36.5 && s.pitch < 47.0, "{}", s.pitch);
        render(&mut s, &[Event::note_off(0, 0, 36, 0)], 0.5);
        assert!((s.pitch - 48.0).abs() < 0.01, "{}", s.pitch);
        assert!(s.on);
        // Let go in between and the next note starts again.
        render(&mut s, &[Event::note_off(0, 0, 48, 0)], 0.1);
        render(&mut s, &[Event::note_on(0, 0, 36, 80, 0)], 0.01);
        assert_eq!(s.pitch, 36.0);
        assert!(s.amp.level > 0.5 || s.amp.held());
    }

    /// The golden render: a fixed patch and line give a known spectrum,
    /// so a change to the toolkit can't quietly change the sound.
    #[test]
    fn a_fixed_patch_renders_to_its_known_spectrum() {
        let mut s = acid();
        s.params.set(at::VINTAGE, 0.0);
        s.params.set(at::WAVE, 1.0);
        s.params.set(at::CUTOFF, 400.0);
        s.params.set(at::RESONANCE, 0.7);
        s.params.set(at::ENV_MOD, 0.6);
        s.params.set(at::DECAY, 0.3);
        let out = render(&mut s, &[Event::note_on(0, 0, 33, 120, 0)], 0.5);
        let f0 = hz(33.0);
        let got: Vec<f32> = [1.0, 3.0, 5.0, 7.0]
            .iter()
            .map(|k| 20.0 * level_at(&out[RATE / 4..], f0 * k).max(1e-9).log10())
            .collect();
        // Measured when the synth was built; a drift of over 1.5 dB at
        // any harmonic means the sound changed.
        let known = [-22.1, -33.6, -35.5, -31.6];
        for (g, k) in got.iter().zip(known) {
            assert!((g - k).abs() < 1.5, "{got:?} vs {known:?}");
        }
    }

    /// The plan's CPU budget, measured in release builds; the bound is
    /// loose because test machines differ (the figure is in the review
    /// notes).
    #[test]
    #[cfg_attr(debug_assertions, ignore = "timed in release builds only")]
    fn a_voice_costs_little() {
        let mut s = acid();
        s.params.set(at::DRIVE, 0.5);
        let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
        let start = std::time::Instant::now();
        s.process(&[Event::note_on(0, 0, 36, 120, 0)], &mut l, &mut r);
        let seconds = 2.0f64;
        for _ in 0..(RATE as f64 * seconds / 512.0) as usize {
            s.process(&[], &mut l, &mut r);
        }
        let used = start.elapsed().as_secs_f64() / seconds;
        eprintln!("one voice: {:.2}% of one core", used * 100.0);
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
            let mut s = Acid::new(RATE as u32, p);
            let x = render(&mut s, &[Event::note_on(0, 0, 45, 110, 0)], 1.0);
            assert!(peak(&x) > 0.02, "{} is silent", sound.name);
            assert!(peak(&x) <= 1.0, "{} clips", sound.name);
        }
    }
}
