// SPDX-License-Identifier: GPL-3.0-or-later
//! Effects that come with the app (roadmap 2.5): EQ, compressor, gate,
//! limiter, delay, an algorithmic reverb, a convolution reverb, a guitar
//! amp, speaker cabinets and a player for amp captures. Each runs as a
//! track insert, like a hosted plugin.
//!
//! An [`Effect`] holds the settings, shared without locks with the
//! [`Processor`] made from it, which runs on the audio thread. Every
//! method here is written from published methods (see each module); none
//! is taken from another product.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use daw_engine::graph::Insert;

mod amp;
mod biquad;
mod cabinet;
pub mod capture;
mod convolution;
mod delay;
mod dynamics;
mod eq;
pub mod library;
mod resample;
pub mod response;
mod reverb;

pub use capture::Capture;
pub use response::Response;

/// The most settings any effect has.
const MAX_PARAMS: usize = 16;
/// Settings are read and smoothed this often, in frames.
const CHUNK: usize = 32;
/// How long a change of setting takes to settle, in seconds.
const SMOOTH_SECONDS: f64 = 0.02;

/// Which effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Eq,
    Compressor,
    Gate,
    Limiter,
    Delay,
    Reverb,
    Convolution,
    Amp,
    Cabinet,
    Capture,
}

impl Kind {
    pub const ALL: [Kind; 10] = [
        Kind::Eq,
        Kind::Compressor,
        Kind::Gate,
        Kind::Limiter,
        Kind::Delay,
        Kind::Reverb,
        Kind::Convolution,
        Kind::Amp,
        Kind::Cabinet,
        Kind::Capture,
    ];

    /// How projects name it. Never change one.
    pub fn id(self) -> &'static str {
        match self {
            Kind::Eq => "eq",
            Kind::Compressor => "compressor",
            Kind::Gate => "gate",
            Kind::Limiter => "limiter",
            Kind::Delay => "delay",
            Kind::Reverb => "reverb",
            Kind::Convolution => "convolution",
            Kind::Amp => "amp",
            Kind::Cabinet => "cabinet",
            Kind::Capture => "amp-capture",
        }
    }

    pub fn from_id(id: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.id() == id)
    }

    /// What the user sees.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Eq => "EQ",
            Kind::Compressor => "Compressor",
            Kind::Gate => "Noise gate",
            Kind::Limiter => "Limiter",
            Kind::Delay => "Delay",
            Kind::Reverb => "Reverb",
            Kind::Convolution => "Room reverb",
            Kind::Amp => "Guitar amp",
            Kind::Cabinet => "Speaker cabinet",
            Kind::Capture => "Amp capture",
        }
    }

    /// A line saying what it does.
    pub fn about(self) -> &'static str {
        match self {
            Kind::Eq => "Cut or boost bass, middle and treble, and cut rumble or hiss",
            Kind::Compressor => "Evens out loud and quiet playing",
            Kind::Gate => "Silences hum and hiss between notes",
            Kind::Limiter => "Stops the level going over a ceiling, for the master or a loud part",
            Kind::Delay => "Echoes",
            Kind::Reverb => "A smooth, adjustable reverb",
            Kind::Convolution => "The sound of a room, made from its echo pattern",
            Kind::Amp => "A guitar amp, from clean to high gain, with a speaker cabinet",
            Kind::Cabinet => "A speaker cabinet, for after an amp or a capture made without one",
            Kind::Capture => "Plays a capture file of a real amp or pedal (.nam) that you supply",
        }
    }

    pub fn params(self) -> &'static [Param] {
        match self {
            Kind::Eq => eq::PARAMS,
            Kind::Compressor => dynamics::COMPRESSOR,
            Kind::Gate => dynamics::GATE,
            Kind::Limiter => dynamics::LIMITER,
            Kind::Delay => delay::PARAMS,
            Kind::Reverb => reverb::PARAMS,
            Kind::Convolution => convolution::PARAMS,
            Kind::Amp => amp::PARAMS,
            Kind::Cabinet => cabinet::PARAMS,
            Kind::Capture => capture::PARAMS,
        }
    }

    /// Whether it shows how much it is turning the level down.
    pub fn has_meter(self) -> bool {
        matches!(self, Kind::Compressor | Kind::Gate | Kind::Limiter)
    }

    /// What kind of file it can use, as [`Effect::set_response`] or
    /// [`Effect::set_capture`] take: none, a response, or a capture.
    pub fn uses(self) -> Uses {
        match self {
            Kind::Convolution | Kind::Cabinet => Uses::Response,
            Kind::Capture => Uses::Capture,
            _ => Uses::Nothing,
        }
    }
}

/// What kind of file an effect can use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Uses {
    Nothing,
    /// An impulse response (a sound file).
    Response,
    /// An amp capture file.
    Capture,
}

/// What a setting's number means, for showing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Db,
    Hz,
    Ms,
    Seconds,
    Percent,
    /// A compression ratio, shown as "4:1".
    Ratio,
    /// A plain number.
    Plain,
    /// One of [`Param::choices`], by index.
    Choice,
    /// Off (0) or on (1).
    Switch,
}

/// One setting of an effect.
#[derive(Clone, Copy, Debug)]
pub struct Param {
    /// How projects name it. Never change one.
    pub key: &'static str,
    pub name: &'static str,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub unit: Unit,
    /// Moves evenly in ratio rather than in steps (frequencies, times).
    pub log: bool,
    /// The value at which it does nothing, shown as "Off".
    pub off_at: Option<f64>,
    pub choices: &'static [&'static str],
}

impl Param {
    pub(crate) const fn new(
        key: &'static str,
        name: &'static str,
        min: f64,
        max: f64,
        default: f64,
        unit: Unit,
    ) -> Param {
        Param {
            key,
            name,
            min,
            max,
            default,
            unit,
            log: false,
            off_at: None,
            choices: &[],
        }
    }

    pub(crate) const fn log(mut self) -> Param {
        self.log = true;
        self
    }

    pub(crate) const fn off_at(mut self, v: f64) -> Param {
        self.off_at = Some(v);
        self
    }

    pub(crate) const fn choices(mut self, c: &'static [&'static str]) -> Param {
        self.choices = c;
        self
    }

    /// Whether it moves in whole steps.
    pub fn stepped(&self) -> bool {
        matches!(self.unit, Unit::Choice | Unit::Switch)
    }

    /// `v` brought into range, and to a whole step where it has them.
    pub fn clamp(&self, v: f64) -> f64 {
        let v = if v.is_finite() { v } else { self.default };
        let v = v.clamp(self.min, self.max);
        if self.stepped() { v.round() } else { v }
    }

    /// `v` for showing, with its unit.
    pub fn text(&self, v: f64) -> String {
        if self.off_at == Some(v) {
            return "Off".into();
        }
        match self.unit {
            Unit::Db => format!("{v:+.1} dB"),
            Unit::Hz if v >= 1000.0 => format!("{:.2} kHz", v / 1000.0),
            Unit::Hz => format!("{v:.0} Hz"),
            Unit::Ms if v < 10.0 => format!("{v:.1} ms"),
            Unit::Ms => format!("{v:.0} ms"),
            Unit::Seconds => format!("{v:.1} s"),
            Unit::Percent => format!("{v:.0}%"),
            Unit::Ratio if v >= 10.0 => format!("{v:.0}:1"),
            Unit::Ratio => format!("{v:.1}:1"),
            Unit::Plain => format!("{v:.2}"),
            Unit::Choice => self.choices.get(v as usize).copied().unwrap_or("?").into(),
            Unit::Switch if v >= 0.5 => "On".into(),
            Unit::Switch => "Off".into(),
        }
    }
}

/// What the settings side and the audio side share.
struct Shared {
    /// Each setting, as `f64` bits would not fit an atomic everywhere:
    /// as `f32` bits.
    values: [AtomicU32; MAX_PARAMS],
    /// How far the effect is turning the level down now, in dB (0 or
    /// below).
    reduction: AtomicU32,
}

/// A built-in effect's settings. Changing one is heard at once by every
/// [`Processor`] made from it.
pub struct Effect {
    kind: Kind,
    shared: Arc<Shared>,
    /// For the convolution reverb, the response it uses instead of one
    /// made here.
    source: Mutex<Source>,
}

/// The file a convolution reverb, cabinet or capture player uses.
#[derive(Clone, Debug, Default)]
struct Source {
    /// As the project saves it: empty for the rooms or cabinets made
    /// here (or no capture), a recorded room's [`library::reference`],
    /// or a file (an asset name, or a full path until the project is
    /// saved).
    reference: String,
    /// The response, if it could be read.
    response: Option<Arc<Response>>,
    /// The capture, if it could be read.
    capture: Option<Arc<Capture>>,
}

impl Effect {
    /// The effect with its default settings.
    pub fn new(kind: Kind) -> Effect {
        let shared = Arc::new(Shared {
            values: std::array::from_fn(|i| {
                AtomicU32::new(
                    kind.params()
                        .get(i)
                        .map_or(0.0, |p| p.default as f32)
                        .to_bits(),
                )
            }),
            reduction: AtomicU32::new(0f32.to_bits()),
        });
        Effect {
            kind,
            shared,
            source: Mutex::default(),
        }
    }

    /// Where its response comes from, as the project saves it (see
    /// [`Effect::set_response`]).
    pub fn response_ref(&self) -> String {
        self.source
            .lock()
            .map(|s| s.reference.clone())
            .unwrap_or_default()
    }

    /// Whether a response named by [`Effect::response_ref`] couldn't be
    /// read, so a room made here plays instead.
    pub fn response_missing(&self) -> bool {
        self.source
            .lock()
            .is_ok_and(|s| !s.reference.is_empty() && s.response.is_none() && s.capture.is_none())
    }

    /// Gives the response's file a new name (once it has come into the
    /// project), keeping what was read from it.
    pub fn rename_response(&self, reference: String) {
        if let Ok(mut s) = self.source.lock() {
            s.reference = reference;
        }
    }

    /// Uses `response` (read from what `reference` names) instead of the
    /// rooms made here. An empty `reference` goes back to them. A
    /// `reference` whose response couldn't be read is kept, so saving
    /// doesn't lose it. Takes effect in the next [`Processor`] made.
    pub fn set_response(&self, reference: String, response: Option<Arc<Response>>) {
        if let Ok(mut s) = self.source.lock() {
            *s = Source {
                reference,
                response,
                capture: None,
            };
        }
    }

    /// Plays `capture` (read from what `reference` names) in a capture
    /// player; an empty `reference` takes it away. As with
    /// [`Effect::set_response`], a `reference` that couldn't be read is
    /// kept, and it takes effect in the next [`Processor`] made.
    pub fn set_capture(&self, reference: String, capture: Option<Arc<Capture>>) {
        if let Ok(mut s) = self.source.lock() {
            *s = Source {
                reference,
                response: None,
                capture,
            };
        }
    }

    /// The capture it plays, if one is read.
    pub fn capture(&self) -> Option<Arc<Capture>> {
        self.source.lock().ok().and_then(|s| s.capture.clone())
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn get(&self, i: usize) -> f64 {
        f32::from_bits(self.shared.values[i].load(Ordering::Relaxed)) as f64
    }

    /// Sets setting `i`, kept in its range.
    pub fn set(&self, i: usize, v: f64) {
        let Some(p) = self.kind.params().get(i) else {
            return;
        };
        let v = p.clamp(v) as f32;
        self.shared.values[i].store(v.to_bits(), Ordering::Relaxed);
    }

    /// Every setting by key, for saving.
    pub fn values(&self) -> Vec<(String, f64)> {
        (self.kind.params().iter().enumerate())
            .map(|(i, p)| (p.key.to_string(), self.get(i)))
            .collect()
    }

    /// Sets settings by key, as saved. Settings not named go back to
    /// their defaults, and keys this version doesn't know are ignored.
    pub fn load(&self, values: &[(String, f64)]) {
        for (i, p) in self.kind.params().iter().enumerate() {
            let v = values
                .iter()
                .find(|(k, _)| k == p.key)
                .map_or(p.default, |(_, v)| *v);
            self.set(i, v);
        }
    }

    /// How far it is turning the level down now, in dB (0 or below).
    pub fn reduction_db(&self) -> f32 {
        f32::from_bits(self.shared.reduction.load(Ordering::Relaxed))
    }

    /// Makes the audio side, at `rate`. Allocates; not for the audio
    /// thread.
    pub fn processor(&self, rate: u32) -> Processor {
        let rate = rate.max(1) as f64;
        let dsp: Box<dyn Dsp> = match self.kind {
            Kind::Eq => Box::new(eq::Eq::new(rate)),
            Kind::Compressor => Box::new(dynamics::Compressor::new(rate)),
            Kind::Gate => Box::new(dynamics::Gate::new(rate)),
            Kind::Limiter => Box::new(dynamics::Limiter::new(rate)),
            Kind::Delay => Box::new(delay::Delay::new(rate)),
            Kind::Reverb => Box::new(reverb::Reverb::new(rate)),
            Kind::Convolution | Kind::Cabinet => {
                let loaded = (self.source.lock().ok())
                    .and_then(|s| s.response.clone())
                    .map(|r| r.at_rate(rate as u32));
                if self.kind == Kind::Cabinet {
                    Box::new(cabinet::Cabinet::new(rate, loaded))
                } else {
                    Box::new(convolution::Convolution::new(rate, loaded))
                }
            }
            Kind::Amp => Box::new(amp::Amp::new(rate)),
            Kind::Capture => Box::new(capture::Player::new(rate, self.capture().as_deref())),
        };
        let mut p = Processor {
            params: self.kind.params(),
            shared: self.shared.clone(),
            target: [0.0; MAX_PARAMS],
            now: [0.0; MAX_PARAMS],
            smooth: 1.0 - (-(CHUNK as f64) / (SMOOTH_SECONDS * rate)).exp(),
            dsp,
            settled: false,
        };
        p.read();
        p.now = p.target;
        p.dsp.update(&p.now[..p.params.len()]);
        p
    }
}

/// The audio side of an effect: what a built-in effect needs from it.
trait Dsp: Send {
    /// Takes new settings, in the order of the effect's params.
    fn update(&mut self, values: &[f64]);
    /// Processes at most [`CHUNK`] frames in place.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]);
    /// Frames of delay it adds.
    fn latency(&self) -> u32 {
        0
    }
    /// How far it is turning the level down now, in dB.
    fn reduction_db(&self) -> f32 {
        0.0
    }
}

/// A built-in effect running on the audio thread.
pub struct Processor {
    params: &'static [Param],
    shared: Arc<Shared>,
    /// The settings as last read, and as smoothed towards them.
    target: [f64; MAX_PARAMS],
    now: [f64; MAX_PARAMS],
    /// How far `now` moves towards `target` each chunk.
    smooth: f64,
    dsp: Box<dyn Dsp>,
    /// `now` has reached `target`.
    settled: bool,
}

impl Processor {
    /// Frames of delay it adds, compensated on the track.
    pub fn latency(&self) -> u32 {
        self.dsp.latency()
    }

    fn read(&mut self) {
        for i in 0..self.params.len() {
            let v = f32::from_bits(self.shared.values[i].load(Ordering::Relaxed)) as f64;
            if v != self.target[i] {
                self.target[i] = v;
                self.settled = false;
            }
        }
    }

    /// Moves the settings a step towards those last read.
    fn glide(&mut self) {
        if self.settled {
            return;
        }
        let mut settled = true;
        for (i, p) in self.params.iter().enumerate() {
            let (t, n) = (self.target[i], &mut self.now[i]);
            if *n == t {
                continue;
            }
            if p.stepped() || p.off_at.is_some_and(|o| o == t || o == *n) {
                // Jumps: there is nothing between two choices, or between
                // off and on.
                *n = t;
                continue;
            }
            let next = if p.log && t > 0.0 && *n > 0.0 {
                (n.ln() + (t.ln() - n.ln()) * self.smooth).exp()
            } else {
                *n + (t - *n) * self.smooth
            };
            if ((next - t) / (p.max - p.min)).abs() < 1e-4 {
                *n = t;
            } else {
                *n = next;
                settled = false;
            }
        }
        self.settled = settled;
        self.dsp.update(&self.now[..self.params.len()]);
    }
}

impl Insert for Processor {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        let mut at = 0;
        while at < n {
            let end = (at + CHUNK).min(n);
            self.read();
            self.glide();
            self.dsp.process(&mut left[at..end], &mut right[at..end]);
            at = end;
        }
        self.shared
            .reduction
            .store(self.dsp.reduction_db().to_bits(), Ordering::Relaxed);
    }
}

/// dB to a gain.
fn gain(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// A gain to dB, with silence at -200 dB.
fn db(g: f64) -> f64 {
    20.0 * g.max(1e-10).log10()
}

/// A one-pole smoothing coefficient for a time constant of `seconds`.
fn coef(seconds: f64, rate: f64) -> f64 {
    if seconds <= 0.0 {
        0.0
    } else {
        (-1.0 / (seconds * rate)).exp()
    }
}

/// The value written `d` frames before the one at `at` in a circular
/// `line`, between frames by straight line.
#[inline]
fn delay_read(line: &[f32], at: usize, d: f64) -> f32 {
    let n = line.len();
    let whole = d.floor();
    let frac = (d - whole) as f32;
    let i = (at + n - (whole as usize % n)) % n;
    let j = (i + n - 1) % n;
    line[i] + (line[j] - line[i]) * frac
}

/// Values too small to hear, flushed so feedback loops never slow down
/// on subnormal numbers.
#[inline]
fn flush(x: f32) -> f32 {
    if x.abs() < 1e-20 { 0.0 } else { x }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_round_trip() {
        for k in Kind::ALL {
            assert_eq!(Kind::from_id(k.id()), Some(k));
            assert!(k.params().len() <= MAX_PARAMS);
            let mut keys: Vec<_> = k.params().iter().map(|p| p.key).collect();
            keys.sort();
            keys.dedup();
            assert_eq!(keys.len(), k.params().len(), "{k:?}");
            for p in k.params() {
                assert!(p.min <= p.default && p.default <= p.max, "{k:?} {}", p.key);
                assert!(!p.log || p.min > 0.0);
            }
        }
        assert_eq!(Kind::from_id("tape"), None);
    }

    #[test]
    fn settings_load_and_save_by_key() {
        let e = Effect::new(Kind::Compressor);
        e.load(&[
            ("ratio".into(), 8.0),
            ("threshold".into(), -400.0),
            ("from-the-future".into(), 1.0),
        ]);
        let v = e.values();
        let get = |k: &str| v.iter().find(|(n, _)| n == k).unwrap().1;
        assert_eq!(get("ratio"), 8.0);
        // Kept in range.
        assert_eq!(get("threshold"), -60.0);
        // Not named: its default.
        assert_eq!(get("attack"), 10.0);
    }

    #[test]
    fn text_shows_units() {
        let p = Param::new("f", "F", 20.0, 20000.0, 20.0, Unit::Hz).off_at(20.0);
        assert_eq!(p.text(20.0), "Off");
        assert_eq!(p.text(2500.0), "2.50 kHz");
        let r = Param::new("r", "R", 1.0, 20.0, 4.0, Unit::Ratio);
        assert_eq!(r.text(4.0), "4.0:1");
    }

    /// Every effect, with its default settings and at its extremes,
    /// keeps finite output on noise, and goes quiet after silence (what
    /// can ring is given time to).
    #[test]
    fn every_effect_is_stable() {
        for k in Kind::ALL {
            for setting in 0..3 {
                let e = Effect::new(k);
                for (i, p) in k.params().iter().enumerate() {
                    match setting {
                        1 => e.set(i, p.min),
                        2 => e.set(i, p.max),
                        _ => {}
                    }
                }
                let mut p = e.processor(48_000);
                let mut seed = 1u32;
                let mut l = vec![0.0f32; 512];
                let mut r = vec![0.0f32; 512];
                for block in 0..400 {
                    for i in 0..512 {
                        let noise = if block < 40 {
                            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                            (seed >> 8) as f32 / (1 << 23) as f32 - 1.0
                        } else {
                            0.0
                        };
                        l[i] = noise * 0.5;
                        r[i] = -noise * 0.5;
                    }
                    p.process(&mut l, &mut r);
                    for x in l.iter().chain(&r) {
                        assert!(
                            x.is_finite() && x.abs() < 64.0,
                            "{k:?} setting {setting}: {x}"
                        );
                    }
                }
                // About four seconds after the noise stopped, with
                // feedback as high as it goes, it may still ring; with
                // the defaults it has died away.
                if setting == 0 {
                    let tail = l.iter().chain(&r).fold(0f32, |m, x| m.max(x.abs()));
                    assert!(tail < 1e-3, "{k:?} still sounds: {tail}");
                }
            }
        }
    }
}

/// How much of one core each effect takes, with its default settings:
/// `cargo test --release -p daw-effects -- --ignored --nocapture cost`.
#[cfg(test)]
#[test]
#[ignore]
fn cost() {
    for k in Kind::ALL {
        let e = Effect::new(k);
        let mut p = e.processor(48_000);
        let mut l = vec![0.1f32; 256];
        let mut r = vec![0.1f32; 256];
        let blocks = 48_000 * 10 / 256;
        let t = std::time::Instant::now();
        for _ in 0..blocks {
            p.process(&mut l, &mut r);
        }
        let share = t.elapsed().as_secs_f64() / 10.0 * 100.0;
        println!("{:<12} {share:.2}% of a core", k.name());
    }
}
