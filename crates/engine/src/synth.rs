// SPDX-License-Identifier: GPL-3.0-or-later
//! A simple synthesiser that comes with the app (roadmap 3.1): enough to
//! hear notes, test the whole path from a keyboard or a clip to the
//! speakers, and sketch a part. One oscillator (sine, triangle, saw or
//! square, the last two band-limited), an attack/decay/sustain/release
//! envelope on the level and a low-pass filter. Sixteen voices; the
//! oldest is taken when all are busy. The pitch wheel bends two
//! semitones each way; the mod wheel adds vibrato; volume (7) and
//! expression (11) set the level and brightness (74) moves the filter.
//! Each note's own expression (roadmap 3.1) bends its pitch, its
//! pressure sets its level and its timbre moves its filter, as does
//! pressure on the whole keyboard for every note on the channel.
//!
//! Its settings are shared with the UI as atomics ([`SynthParams`]), so
//! moving a control while it plays needs no new graph.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::midi::{Event, Expression, Instrument, Kind, pitch_hz};

/// Which waveform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wave {
    Sine,
    Triangle,
    Saw,
    Square,
}

impl Wave {
    pub const ALL: [Wave; 4] = [Wave::Sine, Wave::Triangle, Wave::Saw, Wave::Square];

    pub fn label(self) -> &'static str {
        match self {
            Wave::Sine => "Sine",
            Wave::Triangle => "Triangle",
            Wave::Saw => "Saw",
            Wave::Square => "Square",
        }
    }

    fn from_index(i: u32) -> Wave {
        Wave::ALL.get(i as usize).copied().unwrap_or(Wave::Saw)
    }
}

/// The settings, as the UI and the audio thread share them. Each is a
/// `f32` stored as bits.
pub struct SynthParams {
    wave: AtomicU32,
    attack: AtomicU32,
    decay: AtomicU32,
    sustain: AtomicU32,
    release: AtomicU32,
    cutoff: AtomicU32,
    level_db: AtomicU32,
}

/// The settings' names, as a project stores them, their ranges and
/// defaults: (name, lowest, highest, default).
pub const PARAMS: [(&str, f64, f64, f64); 7] = [
    ("wave", 0.0, 3.0, 2.0),
    ("attack", 0.001, 4.0, 0.005),
    ("decay", 0.001, 4.0, 0.2),
    ("sustain", 0.0, 1.0, 0.7),
    ("release", 0.001, 8.0, 0.3),
    ("cutoff", 50.0, 20_000.0, 6_000.0),
    ("level_db", -36.0, 6.0, -12.0),
];

impl Default for SynthParams {
    fn default() -> SynthParams {
        let p = SynthParams {
            wave: AtomicU32::new(0),
            attack: AtomicU32::new(0),
            decay: AtomicU32::new(0),
            sustain: AtomicU32::new(0),
            release: AtomicU32::new(0),
            cutoff: AtomicU32::new(0),
            level_db: AtomicU32::new(0),
        };
        for (name, _, _, default) in PARAMS {
            p.set(name, default);
        }
        p
    }
}

impl SynthParams {
    fn slot(&self, name: &str) -> Option<&AtomicU32> {
        Some(match name {
            "wave" => &self.wave,
            "attack" => &self.attack,
            "decay" => &self.decay,
            "sustain" => &self.sustain,
            "release" => &self.release,
            "cutoff" => &self.cutoff,
            "level_db" => &self.level_db,
            _ => return None,
        })
    }

    /// Sets a setting by name, within its range. Unknown names are
    /// ignored.
    pub fn set(&self, name: &str, value: f64) {
        let Some((_, lo, hi, _)) = PARAMS.iter().find(|p| p.0 == name) else {
            return;
        };
        let v = if value.is_finite() {
            value.clamp(*lo, *hi)
        } else {
            *lo
        };
        if let Some(s) = self.slot(name) {
            s.store((v as f32).to_bits(), Ordering::Relaxed);
        }
    }

    pub fn get(&self, name: &str) -> f64 {
        self.slot(name)
            .map_or(0.0, |s| f32::from_bits(s.load(Ordering::Relaxed)) as f64)
    }

    /// Every setting by name, as a project stores them.
    pub fn all(&self) -> Vec<(String, f64)> {
        PARAMS
            .iter()
            .map(|(n, ..)| (n.to_string(), self.get(n)))
            .collect()
    }

    /// Sets every setting given; the rest keep their values.
    pub fn set_all(&self, params: &[(String, f64)]) {
        for (k, v) in params {
            self.set(k, *v);
        }
    }

    fn read(&self, s: &AtomicU32) -> f32 {
        f32::from_bits(s.load(Ordering::Relaxed))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Off,
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Clone, Copy)]
struct Voice {
    stage: Stage,
    channel: u8,
    pitch: u8,
    /// The note's id, for its expression.
    id: u32,
    /// The note's own expression: semitones of tuning, pressure (as a
    /// level, 1 when none came) and timbre (0.5 as set).
    tune: f32,
    pressure: f32,
    timbre: f32,
    /// Oscillator phase, 0 to 1.
    phase: f32,
    /// Envelope level, 0 to 1.
    env: f32,
    velocity: f32,
    /// Filter state (two one-pole stages).
    lp: [f32; 2],
    /// When it started, for stealing the oldest.
    age: u64,
}

impl Voice {
    const OFF: Voice = Voice {
        stage: Stage::Off,
        channel: 0,
        pitch: 0,
        id: 0,
        tune: 0.0,
        pressure: 1.0,
        timbre: 0.5,
        phase: 0.0,
        env: 0.0,
        velocity: 0.0,
        lp: [0.0; 2],
        age: 0,
    };
}

const VOICES: usize = 16;

pub struct SimpleSynth {
    params: Arc<SynthParams>,
    rate: f32,
    voices: [Voice; VOICES],
    /// Pitch bend per channel, in semitones.
    bend: [f32; 16],
    /// Sustain pedal per channel: notes released while it is down wait.
    pedal: [bool; 16],
    held: [[bool; 128]; 16],
    /// Per channel: the level from volume and expression (and pressure
    /// on the whole keyboard), the filter's shift in octaves from
    /// brightness, and the mod wheel's vibrato depth in semitones.
    level: [[f32; 3]; 16],
    bright: [f32; 16],
    vibrato: [f32; 16],
    /// The vibrato's phase, 0 to 1.
    lfo: f32,
    clock: u64,
}

impl SimpleSynth {
    pub fn new(rate: u32, params: Arc<SynthParams>) -> SimpleSynth {
        SimpleSynth {
            params,
            rate: rate.max(1) as f32,
            voices: [Voice::OFF; VOICES],
            bend: [0.0; 16],
            pedal: [false; 16],
            held: [[false; 128]; 16],
            level: [[1.0; 3]; 16],
            bright: [0.0; 16],
            vibrato: [0.0; 16],
            lfo: 0.0,
            clock: 0,
        }
    }

    pub fn params(&self) -> &Arc<SynthParams> {
        &self.params
    }

    fn note_on(&mut self, channel: u8, pitch: u8, velocity: u8, id: u32) {
        self.clock += 1;
        let (ch, p) = (channel as usize & 15, pitch as usize & 127);
        self.held[ch][p] = true;
        // A free voice, else the oldest.
        let slot = (self.voices.iter().position(|v| v.stage == Stage::Off))
            .unwrap_or_else(|| (0..VOICES).min_by_key(|&i| self.voices[i].age).unwrap_or(0));
        let v = &mut self.voices[slot];
        let keep_env = v.stage != Stage::Off;
        *v = Voice {
            stage: Stage::Attack,
            channel: ch as u8,
            pitch: p as u8,
            id,
            tune: 0.0,
            pressure: 1.0,
            timbre: 0.5,
            phase: if keep_env { v.phase } else { 0.0 },
            env: if keep_env { v.env } else { 0.0 },
            velocity: velocity.clamp(1, 127) as f32 / 127.0,
            lp: if keep_env { v.lp } else { [0.0; 2] },
            age: self.clock,
        };
    }

    /// Releases the voices at this pitch on this channel. The note id is
    /// not needed: a new graph numbers notes afresh, and MIDI itself has
    /// no ids.
    fn note_off(&mut self, channel: u8, pitch: u8) {
        let (ch, p) = (channel as usize & 15, pitch as usize & 127);
        self.held[ch][p] = false;
        if self.pedal[ch] {
            return;
        }
        for v in &mut self.voices {
            if v.stage != Stage::Off
                && v.stage != Stage::Release
                && v.channel == ch as u8
                && v.pitch == p as u8
            {
                v.stage = Stage::Release;
            }
        }
    }

    fn all_off(&mut self) {
        for v in &mut self.voices {
            if v.stage != Stage::Off {
                v.stage = Stage::Release;
            }
        }
        self.held = [[false; 128]; 16];
        self.pedal = [false; 16];
    }

    fn event(&mut self, e: &Event) {
        match e.kind {
            Kind::NoteOn {
                pitch,
                velocity,
                id,
            } => self.note_on(e.channel, pitch, velocity, id),
            Kind::NoteExpression {
                id,
                pitch,
                kind,
                value,
            } => {
                for v in (self.voices.iter_mut()).filter(|v| {
                    v.stage != Stage::Off && v.id == id && v.pitch == pitch && v.channel == e.channel
                }) {
                    match kind {
                        Expression::Tuning => v.tune = value.clamp(-48.0, 48.0),
                        Expression::Pressure => v.pressure = value.clamp(0.0, 1.0),
                        Expression::Timbre => v.timbre = value.clamp(0.0, 1.0),
                    }
                }
            }
            Kind::Control { number, value } if matches!(number, 1 | 7 | 11 | 74) => {
                let ch = e.channel as usize & 15;
                let v = value as f32 / 127.0;
                match number {
                    1 => self.vibrato[ch] = v * 0.5,
                    7 => self.level[ch][0] = v * v,
                    11 => self.level[ch][1] = v * v,
                    _ => self.bright[ch] = (value as f32 - 64.0) / 64.0 * 2.0,
                }
            }
            Kind::Pressure(value) => {
                self.level[e.channel as usize & 15][2] = value as f32 / 127.0;
            }
            Kind::NoteOff { pitch, .. } => self.note_off(e.channel, pitch),
            Kind::PitchBend(b) => self.bend[e.channel as usize & 15] = b as f32 / 8192.0 * 2.0,
            Kind::Control { number: 64, value } => {
                let ch = e.channel as usize & 15;
                self.pedal[ch] = value >= 64;
                if !self.pedal[ch] {
                    for v in &mut self.voices {
                        if v.channel == ch as u8
                            && v.stage != Stage::Off
                            && v.stage != Stage::Release
                            && !self.held[ch][v.pitch as usize]
                        {
                            v.stage = Stage::Release;
                        }
                    }
                }
            }
            Kind::Control { number: 120, .. } | Kind::AllNotesOff => self.all_off(),
            Kind::Control { number: 121, .. } => {
                // Reset all controllers.
                let ch = e.channel as usize & 15;
                self.level[ch] = [1.0; 3];
                self.bright[ch] = 0.0;
                self.vibrato[ch] = 0.0;
                self.bend[ch] = 0.0;
            }
            _ => {}
        }
    }

    /// Renders frames `[from, to)` of the block for every voice.
    fn run(&mut self, from: usize, to: usize, left: &mut [f32], right: &mut [f32]) {
        if to <= from {
            return;
        }
        let p = &self.params;
        let wave = Wave::from_index(p.read(&p.wave) as u32);
        let rate = self.rate;
        let attack = 1.0 / (p.read(&p.attack).max(0.0005) * rate);
        let decay = 1.0 / (p.read(&p.decay).max(0.0005) * rate);
        let sustain = p.read(&p.sustain).clamp(0.0, 1.0);
        let release = 1.0 / (p.read(&p.release).max(0.0005) * rate);
        let cutoff = p.read(&p.cutoff);
        let gain = 10f32.powf(p.read(&p.level_db) / 20.0);
        // The vibrato for this stretch, at 5.5 Hz, taken at its middle.
        let lfo = (self.lfo + 5.5 * (to - from) as f32 / rate / 2.0).fract();
        let wobble = (lfo * std::f32::consts::TAU).sin();
        self.lfo = (self.lfo + 5.5 * (to - from) as f32 / rate).fract();
        for v in &mut self.voices {
            if v.stage == Stage::Off {
                continue;
            }
            let ch = v.channel as usize;
            let semis = self.bend[ch] + v.tune + self.vibrato[ch] * wobble;
            let hz = pitch_hz(v.pitch as f64 + semis as f64, 440.0) as f32;
            let dt = hz / rate;
            // One-pole coefficient for the cutoff (moved by brightness
            // and the note's timbre, two octaves each way), applied
            // twice.
            let shift = self.bright[ch] + (v.timbre - 0.5) * 4.0;
            let cut = (cutoff * 2f32.powf(shift)).clamp(20.0, rate * 0.45);
            let k = 1.0 - (-2.0 * std::f32::consts::PI * cut / rate).exp();
            let [vol, expr, press] = self.level[ch];
            let gain = gain * vol * expr * press * v.pressure;
            for i in from..to {
                match v.stage {
                    Stage::Attack => {
                        v.env += attack;
                        if v.env >= 1.0 {
                            v.env = 1.0;
                            v.stage = Stage::Decay;
                        }
                    }
                    Stage::Decay => {
                        v.env -= decay * (1.0 - sustain).max(0.001);
                        if v.env <= sustain {
                            v.env = sustain;
                            v.stage = Stage::Sustain;
                        }
                    }
                    Stage::Sustain => {}
                    Stage::Release => {
                        v.env -= release;
                        if v.env <= 0.0 {
                            v.env = 0.0;
                            v.stage = Stage::Off;
                            break;
                        }
                    }
                    Stage::Off => break,
                }
                let x = oscillate(wave, v.phase, dt);
                v.phase += dt;
                if v.phase >= 1.0 {
                    v.phase -= 1.0;
                }
                v.lp[0] += k * (x - v.lp[0]);
                v.lp[1] += k * (v.lp[0] - v.lp[1]);
                let s = v.lp[1] * v.env * v.velocity * gain;
                left[i] += s;
                right[i] += s;
            }
        }
    }
}

/// One sample of the waveform at `phase` (0 to 1), with polyBLEP
/// smoothing of the saw's and square's jumps for a step of `dt` per
/// sample.
fn oscillate(wave: Wave, phase: f32, dt: f32) -> f32 {
    match wave {
        Wave::Sine => (phase * std::f32::consts::TAU).sin(),
        Wave::Triangle => {
            let x = 4.0 * phase;
            if phase < 0.5 { x - 1.0 } else { 3.0 - x }
        }
        Wave::Saw => 2.0 * phase - 1.0 - poly_blep(phase, dt),
        Wave::Square => {
            let s = if phase < 0.5 { 1.0 } else { -1.0 };
            s + poly_blep(phase, dt) - poly_blep((phase + 0.5) % 1.0, dt)
        }
    }
}

/// The correction that rounds off a jump at phase 0 (PolyBLEP).
fn poly_blep(t: f32, dt: f32) -> f32 {
    if dt <= 0.0 {
        0.0
    } else if t < dt {
        let t = t / dt;
        t + t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

impl Instrument for SimpleSynth {
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

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn a_note_sounds_from_its_frame_and_dies_away_after_release() {
        let p = Arc::new(SynthParams::default());
        p.set("release", 0.01);
        let mut s = SimpleSynth::new(48_000, p);
        let (mut l, mut r) = (vec![0.0; 1000], vec![0.0; 1000]);
        s.process(&[Event::note_on(500, 0, 69, 100, 1)], &mut l, &mut r);
        assert_eq!(peak(&l[..500]), 0.0);
        assert!(peak(&l[500..]) > 0.01);
        assert_eq!(l, r);
        l.fill(0.0);
        s.process(&[Event::note_off(0, 0, 69, 1)], &mut l, &mut r);
        assert!(peak(&l[..100]) > 0.0);
        let (mut l, mut r) = (vec![0.0; 1000], vec![0.0; 1000]);
        s.process(&[], &mut l, &mut r);
        assert_eq!(peak(&l[600..]), 0.0, "silent once released");
    }

    #[test]
    fn the_pitch_is_the_notes() {
        let p = Arc::new(SynthParams::default());
        p.set("wave", 0.0);
        p.set("attack", 0.001);
        p.set("cutoff", 20_000.0);
        let mut s = SimpleSynth::new(48_000, p);
        let (mut l, mut r) = (vec![0.0; 48_000], vec![0.0; 48_000]);
        s.process(&[Event::note_on(0, 0, 69, 127, 1)], &mut l, &mut r);
        // Zero crossings over the last half second: 440 Hz gives 220
        // cycles, two crossings each.
        let tail = &l[24_000..];
        let crossings = tail
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        assert!((215..=225).contains(&(crossings / 2)), "{crossings}");
    }

    #[test]
    fn a_notes_expression_bends_it_and_pressure_sets_its_level() {
        let p = Arc::new(SynthParams::default());
        p.set("wave", 0.0);
        p.set("attack", 0.001);
        p.set("cutoff", 20_000.0);
        let mut s = SimpleSynth::new(48_000, p);
        let (mut l, mut r) = (vec![0.0; 48_000], vec![0.0; 48_000]);
        let x = |kind, value| Event {
            frame: 0,
            channel: 0,
            kind: Kind::NoteExpression {
                id: 9,
                pitch: 69,
                kind,
                value,
            },
        };
        s.process(
            &[
                Event::note_on(0, 0, 69, 127, 9),
                // An octave up: 880 Hz.
                x(Expression::Tuning, 12.0),
            ],
            &mut l,
            &mut r,
        );
        let tail = &l[24_000..];
        let crossings = tail
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        assert!((435..=445).contains(&(crossings / 2)), "{crossings}");
        let loud = peak(tail);
        // Another note's expression changes nothing.
        let other = Event {
            kind: Kind::NoteExpression {
                id: 3,
                pitch: 69,
                kind: Expression::Pressure,
                value: 0.0,
            },
            ..x(Expression::Pressure, 0.0)
        };
        l.fill(0.0);
        s.process(&[other], &mut l, &mut r);
        assert!((peak(&l[1000..]) - loud).abs() < loud * 0.05);
        l.fill(0.0);
        s.process(&[x(Expression::Pressure, 0.25)], &mut l, &mut r);
        let soft = peak(&l[1000..]);
        assert!((soft / loud - 0.25).abs() < 0.03, "{soft} {loud}");
        // Volume on the channel does the same for every note.
        l.fill(0.0);
        let vol = Event {
            frame: 0,
            channel: 0,
            kind: Kind::Control {
                number: 7,
                value: 0,
            },
        };
        s.process(&[vol], &mut l, &mut r);
        assert!(peak(&l[1000..]) < 1e-6);
    }

    #[test]
    fn settings_round_trip_by_name() {
        let p = SynthParams::default();
        p.set("cutoff", 1234.0);
        p.set("made up", 1.0);
        p.set("sustain", 7.0);
        assert_eq!(p.get("cutoff"), 1234.0);
        assert_eq!(p.get("sustain"), 1.0);
        let all = p.all();
        let q = SynthParams::default();
        q.set_all(&all);
        assert_eq!(q.all(), all);
    }
}
