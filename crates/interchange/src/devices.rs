// SPDX-License-Identifier: GPL-3.0-or-later
//! Devices: how our effects, instruments and plugins are written, and
//! read back.
//!
//! - **The four generic devices.** The format has an EQ, a compressor,
//!   a gate and a limiter that every DAW can map to its own. Ours are
//!   written as those, setting by setting (tables below), and read back
//!   the same way. Settings the generic device has no place for are
//!   listed in the report.
//! - **Our own devices.** Every built-in effect or instrument (the four
//!   above too) carries its full settings in our own format as the
//!   device's state file, under a device id starting [`OWN`]. Reading a
//!   file we wrote, that state is used, so nothing is lost coming back.
//! - **Plugins.** VST 3 settings go in a `.vstpreset` file, as the
//!   format asks; CLAP settings go in a `.clap-preset` file holding the
//!   plugin's saved state as it gave it.

use daw_effects::Kind;
use daw_project::{Effect, Project, Track, template_from_bytes, template_to_bytes};

/// Device ids of our own devices start with this, then the built-in's
/// id: `daw:reverb`.
pub const OWN: &str = "daw:";

/// One of the format's generic devices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Generic {
    Eq,
    Compressor,
    Gate,
    Limiter,
}

impl Generic {
    pub fn of(builtin: &str) -> Option<Generic> {
        match builtin {
            "eq" => Some(Generic::Eq),
            "compressor" => Some(Generic::Compressor),
            "gate" => Some(Generic::Gate),
            "limiter" => Some(Generic::Limiter),
            _ => None,
        }
    }

    /// The format's element.
    pub fn element(self) -> &'static str {
        match self {
            Generic::Eq => "Equalizer",
            Generic::Compressor => "Compressor",
            Generic::Gate => "NoiseGate",
            Generic::Limiter => "Limiter",
        }
    }

    pub fn from_element(name: &str) -> Option<Generic> {
        [
            Generic::Eq,
            Generic::Compressor,
            Generic::Gate,
            Generic::Limiter,
        ]
        .into_iter()
        .find(|g| g.element() == name)
    }

    pub fn builtin(self) -> &'static str {
        match self {
            Generic::Eq => "eq",
            Generic::Compressor => "compressor",
            Generic::Gate => "gate",
            Generic::Limiter => "limiter",
        }
    }

    /// The settings that map one to one, in the order the format's
    /// schema has the elements. The EQ's are in [`EQ_BANDS`].
    pub fn params(self) -> &'static [GenParam] {
        match self {
            Generic::Eq => EQ_OUTPUT,
            Generic::Compressor => COMPRESSOR,
            Generic::Gate => GATE,
            Generic::Limiter => LIMITER,
        }
    }

    /// Our settings the generic device has no place for.
    pub fn unmapped(self) -> &'static [&'static str] {
        match self {
            Generic::Eq => &[],
            Generic::Compressor => &["knee", "mix"],
            Generic::Gate => &["hold"],
            Generic::Limiter => &[],
        }
    }
}

/// A generic device's setting and ours: the element, our setting's key,
/// the format's unit, and the conversions from ours and back.
pub struct GenParam {
    pub element: &'static str,
    pub key: &'static str,
    pub unit: &'static str,
    pub to: fn(f64) -> f64,
    pub from: fn(f64) -> f64,
}

fn same(v: f64) -> f64 {
    v
}
fn ms_to_s(v: f64) -> f64 {
    v / 1000.0
}
fn s_to_ms(v: f64) -> f64 {
    v * 1000.0
}
fn neg(v: f64) -> f64 {
    -v
}
/// A ratio as the format's percent: how much of the level above the
/// threshold is taken away, so 1:1 is 0%, 4:1 is 75% and ∞:1 is 100%.
fn ratio_to_pct(r: f64) -> f64 {
    (1.0 - 1.0 / r.max(1.0)) * 100.0
}
fn pct_to_ratio(p: f64) -> f64 {
    let p = p.clamp(0.0, 99.99);
    1.0 / (1.0 - p / 100.0)
}

const fn gp(
    element: &'static str,
    key: &'static str,
    unit: &'static str,
    to: fn(f64) -> f64,
    from: fn(f64) -> f64,
) -> GenParam {
    GenParam {
        element,
        key,
        unit,
        to,
        from,
    }
}

const COMPRESSOR: &[GenParam] = &[
    gp("Attack", "attack", "seconds", ms_to_s, s_to_ms),
    gp("OutputGain", "makeup", "decibel", same, same),
    gp("Ratio", "ratio", "percent", ratio_to_pct, pct_to_ratio),
    gp("Release", "release", "seconds", ms_to_s, s_to_ms),
    gp("Threshold", "threshold", "decibel", same, same),
];

const GATE: &[GenParam] = &[
    gp("Attack", "attack", "seconds", ms_to_s, s_to_ms),
    gp("Range", "depth", "decibel", neg, neg),
    gp("Release", "release", "seconds", ms_to_s, s_to_ms),
    gp("Threshold", "threshold", "decibel", same, same),
];

const LIMITER: &[GenParam] = &[
    gp("InputGain", "drive", "decibel", same, same),
    gp("Release", "release", "seconds", ms_to_s, s_to_ms),
    gp("Threshold", "ceiling", "decibel", same, same),
];

const EQ_OUTPUT: &[GenParam] = &[gp("OutputGain", "output", "decibel", same, same)];

/// One band of our EQ as a band of the generic one.
pub struct EqBand {
    pub kind: &'static str,
    pub freq: &'static str,
    pub gain: Option<&'static str>,
    pub q: Option<&'static str>,
    /// For a cut, the frequency at which it is off.
    pub off_at: Option<f64>,
}

/// Our EQ's six bands, low to high.
pub const EQ_BANDS: [EqBand; 6] = [
    EqBand {
        kind: "highPass",
        freq: "low_cut",
        gain: None,
        q: None,
        off_at: Some(20.0),
    },
    EqBand {
        kind: "lowShelf",
        freq: "low_freq",
        gain: Some("low_gain"),
        q: None,
        off_at: None,
    },
    EqBand {
        kind: "bell",
        freq: "mid1_freq",
        gain: Some("mid1_gain"),
        q: Some("mid1_q"),
        off_at: None,
    },
    EqBand {
        kind: "bell",
        freq: "mid2_freq",
        gain: Some("mid2_gain"),
        q: Some("mid2_q"),
        off_at: None,
    },
    EqBand {
        kind: "highShelf",
        freq: "high_freq",
        gain: Some("high_gain"),
        q: None,
        off_at: None,
    },
    EqBand {
        kind: "lowPass",
        freq: "high_cut",
        gain: None,
        q: None,
        off_at: Some(20_000.0),
    },
];

/// A built-in's setting: as the effect has it, or its default.
pub fn value(e: &Effect, key: &str) -> f64 {
    if let Some((_, v)) = e.params.iter().find(|(k, _)| k == key) {
        return *v;
    }
    Kind::from_id(&e.builtin)
        .and_then(|k| k.params().iter().find(|p| p.key == key))
        .map_or(0.0, |p| p.default)
}

/// The lowest and highest a built-in's setting can be.
pub fn range(builtin: &str, key: &str) -> Option<(f64, f64)> {
    let k = Kind::from_id(builtin)?;
    let p = k.params().iter().find(|p| p.key == key)?;
    Some((p.min, p.max))
}

/// Whether a built-in's setting is at its default.
pub fn is_default(e: &Effect, key: &str) -> bool {
    let d = Kind::from_id(&e.builtin)
        .and_then(|k| k.params().iter().find(|p| p.key == key))
        .map(|p| p.default);
    d.is_none_or(|d| (value(e, key) - d).abs() < 1e-9)
}

/// Sets a built-in's setting.
pub fn set(e: &mut Effect, key: &str, v: f64) {
    match e.params.iter_mut().find(|(k, _)| k == key) {
        Some(p) => p.1 = v,
        None => e.params.push((key.into(), v)),
    }
}

/// What the format calls the unit of one of our settings, for a device
/// of our own whose settings are listed for automation.
pub fn unit_of(builtin: &str, key: &str) -> &'static str {
    use daw_effects::Unit;
    let unit = Kind::from_id(builtin)
        .and_then(|k| k.params().iter().find(|p| p.key == key))
        .map(|p| p.unit);
    match unit {
        Some(Unit::Db) => "decibel",
        Some(Unit::Hz) => "hertz",
        Some(Unit::Percent) => "percent",
        Some(Unit::Seconds) => "seconds",
        _ => "linear",
    }
}

/// Changes every sound file an effect names (its response, an
/// instrument's sounds, and those of the effects inside it).
pub fn map_files(e: &mut Effect, f: &mut dyn FnMut(&str) -> String) {
    if e.asset().is_some() {
        e.response = f(&e.response);
    }
    for (_, r) in &mut e.samples {
        if !r.is_empty() && !r.starts_with("made:") {
            *r = f(r);
        }
    }
    for i in &mut e.inner {
        map_files(i, f);
    }
}

/// An effect's full settings in our own format: a template holding one
/// track with just this effect.
pub fn own_state(e: &Effect) -> Vec<u8> {
    let mut t = Track::new(String::new(), String::new());
    t.effects.push(e.clone());
    template_to_bytes(&Project {
        tracks: vec![t],
        ..Project::default()
    })
}

/// The effect [`own_state`] wrote.
pub fn from_own_state(bytes: &[u8]) -> Option<Effect> {
    let p = template_from_bytes(bytes).ok()?;
    p.tracks.into_iter().next()?.effects.into_iter().next()
}

/// A VST 3 class id as the 32 hex digits other hosts write, from the
/// bytes the project keeps (in the platform's order).
pub fn class_hex(class: &[u8]) -> Option<String> {
    let raw: String = class.iter().map(|b| format!("{b:02X}")).collect();
    daw_plugins::cid_from_hex(&raw).map(|t| daw_plugins::presets::canonical_hex(&t))
}

/// The bytes the project keeps for the class id written as `hex`.
pub fn class_from_hex(hex: &str) -> Option<Vec<u8>> {
    let hex = hex.trim().trim_start_matches('{').trim_end_matches('}');
    let hex: String = hex.chars().filter(|c| *c != '-').collect();
    if hex.len() != 32 {
        return None;
    }
    let mut b = [0u8; 16];
    for (i, v) in b.iter_mut().enumerate() {
        *v = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    // On Windows the first three groups are kept the other way round
    // (see `canonical_hex`); undoing it twice is doing nothing.
    if cfg!(windows) {
        b[0..4].reverse();
        b.swap(4, 5);
        b.swap(6, 7);
    }
    Some(b.to_vec())
}

/// The class id the project keeps for a CLAP plugin with this id.
pub fn clap_class(id: &str) -> Vec<u8> {
    daw_plugins::clap::cid_of(id)
        .iter()
        .map(|&b| b as u8)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_ids_go_out_and_come_back_the_same() {
        let class: Vec<u8> = (0..16).map(|i| i * 17).collect();
        let hex = class_hex(&class).unwrap();
        assert_eq!(hex.len(), 32);
        assert_eq!(class_from_hex(&hex), Some(class.clone()));
        assert_eq!(class_from_hex(&hex.to_lowercase()), Some(class));
        assert_eq!(class_from_hex("12"), None);
    }

    #[test]
    fn ratios_map_to_the_formats_percent_and_back() {
        assert_eq!(ratio_to_pct(1.0), 0.0);
        assert_eq!(ratio_to_pct(4.0), 75.0);
        assert!((pct_to_ratio(75.0) - 4.0).abs() < 1e-12);
        assert!((pct_to_ratio(ratio_to_pct(20.0)) - 20.0).abs() < 1e-9);
        assert!(pct_to_ratio(100.0).is_finite());
    }

    #[test]
    fn our_own_state_keeps_everything() {
        let mut e = Effect {
            id: "x".into(),
            name: "Room".into(),
            builtin: "reverb".into(),
            params: vec![("size".into(), 0.7)],
            response: "/tmp/hall.wav".into(),
            samples: vec![("pad1".into(), "made:kick".into())],
            ..Effect::default()
        };
        map_files(&mut e, &mut |r| format!("zip:{r}"));
        assert_eq!(e.response, "zip:/tmp/hall.wav");
        assert_eq!(e.samples[0].1, "made:kick");
        let back = from_own_state(&own_state(&e)).unwrap();
        assert_eq!(back.builtin, "reverb");
        assert_eq!(back.params, e.params);
        assert_eq!(back.response, e.response);
    }

    #[test]
    fn settings_fall_back_to_their_defaults() {
        let e = Effect {
            builtin: "compressor".into(),
            params: vec![("ratio".into(), 8.0)],
            ..Effect::default()
        };
        assert_eq!(value(&e, "ratio"), 8.0);
        assert_eq!(value(&e, "threshold"), -18.0);
        assert!(is_default(&e, "knee"));
        assert!(!is_default(&e, "ratio"));
        assert_eq!(range("compressor", "ratio"), Some((1.0, 20.0)));
    }
}
