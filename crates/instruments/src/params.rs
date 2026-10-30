// SPDX-License-Identifier: GPL-3.0-or-later
//! Instrument settings, shared without locks between the controls and
//! the audio thread: each a number in a range, stored as `f32` bits in
//! an atomic, and saved by key.

use std::sync::atomic::{AtomicU32, Ordering};

/// What a setting's number means, for showing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Db,
    Hz,
    Seconds,
    Semitones,
    Octaves,
    Cents,
    /// 0 to 1, shown as a percentage.
    Amount,
    /// −1 (left) to +1 (right).
    Pan,
    /// A MIDI note number, shown as a note name.
    Note,
    /// A whole number.
    Count,
    /// One of [`Param::choices`], by index.
    Choice,
    /// Off (0) or on (1).
    Switch,
    /// Kept for saving, not shown (a slice's place in its loop).
    Hidden,
}

/// One setting.
#[derive(Clone, Debug)]
pub struct Param {
    /// How projects name it. Never change one.
    pub key: String,
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub unit: Unit,
    /// Moves evenly in ratio rather than in steps (frequencies, times).
    pub log: bool,
    pub choices: &'static [&'static str],
}

impl Param {
    pub fn new(
        key: impl Into<String>,
        name: impl Into<String>,
        min: f64,
        max: f64,
        default: f64,
        unit: Unit,
    ) -> Param {
        Param {
            key: key.into(),
            name: name.into(),
            min,
            max,
            default,
            unit,
            log: false,
            choices: &[],
        }
    }

    pub fn log(mut self) -> Param {
        self.log = true;
        self
    }

    pub fn choices(mut self, c: &'static [&'static str]) -> Param {
        self.choices = c;
        self.unit = Unit::Choice;
        self.max = (c.len().max(1) - 1) as f64;
        self
    }

    /// Whether it moves in whole steps.
    pub fn stepped(&self) -> bool {
        matches!(
            self.unit,
            Unit::Choice | Unit::Switch | Unit::Note | Unit::Count | Unit::Hidden
        )
    }

    /// `v` brought into range, and to a whole step where it has them.
    pub fn clamp(&self, v: f64) -> f64 {
        let v = if v.is_finite() { v } else { self.default };
        let v = v.clamp(self.min, self.max);
        if self.stepped() { v.round() } else { v }
    }

    /// `v` for showing, with its unit.
    pub fn text(&self, v: f64) -> String {
        match self.unit {
            Unit::Db if v <= -60.0 => "Off".into(),
            Unit::Db => format!("{v:+.1} dB"),
            Unit::Hz if v >= 1000.0 => format!("{:.2} kHz", v / 1000.0),
            Unit::Hz => format!("{v:.1} Hz"),
            Unit::Seconds if v < 1.0 => format!("{:.0} ms", v * 1000.0),
            Unit::Seconds => format!("{v:.2} s"),
            Unit::Semitones => format!("{v:+.1} st"),
            Unit::Octaves => format!("{v:+.2} oct"),
            Unit::Cents => format!("{v:+.0} ct"),
            Unit::Amount => format!("{:.0}%", v * 100.0),
            Unit::Pan if v.abs() < 0.005 => "Centre".into(),
            Unit::Pan if v < 0.0 => format!("{:.0} L", -v * 100.0),
            Unit::Pan => format!("{:.0} R", v * 100.0),
            Unit::Note => note_name(v as i32),
            Unit::Count | Unit::Hidden => format!("{v:.0}"),
            Unit::Choice => self.choices.get(v as usize).copied().unwrap_or("?").into(),
            Unit::Switch if v >= 0.5 => "On".into(),
            Unit::Switch => "Off".into(),
        }
    }
}

/// A note number as a name: 60 is C4.
pub fn note_name(n: i32) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!(
        "{}{}",
        NAMES[n.rem_euclid(12) as usize],
        n.div_euclid(12) - 1
    )
}

/// An instrument's settings.
pub struct Params {
    list: Vec<Param>,
    values: Vec<AtomicU32>,
}

impl Params {
    /// The settings in `list`, each at its default.
    pub fn new(list: Vec<Param>) -> Params {
        let values = (list.iter())
            .map(|p| AtomicU32::new((p.default as f32).to_bits()))
            .collect();
        Params { list, values }
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn list(&self) -> &[Param] {
        &self.list
    }

    pub fn index(&self, key: &str) -> Option<usize> {
        self.list.iter().position(|p| p.key == key)
    }

    /// Setting `i`, as the audio thread reads it.
    #[inline]
    pub fn read(&self, i: usize) -> f32 {
        self.values
            .get(i)
            .map_or(0.0, |v| f32::from_bits(v.load(Ordering::Relaxed)))
    }

    pub fn get(&self, i: usize) -> f64 {
        self.read(i) as f64
    }

    /// Sets setting `i`, kept in its range.
    pub fn set(&self, i: usize, v: f64) {
        if let (Some(p), Some(slot)) = (self.list.get(i), self.values.get(i)) {
            slot.store((p.clamp(v) as f32).to_bits(), Ordering::Relaxed);
        }
    }

    pub fn get_key(&self, key: &str) -> Option<f64> {
        self.index(key).map(|i| self.get(i))
    }

    pub fn set_key(&self, key: &str, v: f64) {
        if let Some(i) = self.index(key) {
            self.set(i, v);
        }
    }

    /// Every setting by key, for saving.
    pub fn values(&self) -> Vec<(String, f64)> {
        (self.list.iter().enumerate())
            .map(|(i, p)| (p.key.clone(), self.get(i)))
            .collect()
    }

    /// Sets the settings named; the rest keep their values, and keys
    /// this version doesn't know are ignored.
    pub fn load(&self, values: &[(String, f64)]) {
        for (k, v) in values {
            self.set_key(k, *v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_stay_in_range_and_round_trip_by_key() {
        let p = Params::new(vec![
            Param::new("a", "A", 0.0, 1.0, 0.5, Unit::Amount),
            Param::new("b", "B", 0.0, 0.0, 0.0, Unit::Choice).choices(&["x", "y", "z"]),
        ]);
        p.set(0, 7.0);
        p.set(1, 1.4);
        assert_eq!(p.get(0), 1.0);
        assert_eq!(p.get(1), 1.0);
        assert_eq!(p.list()[1].text(1.0), "y");
        let q = Params::new(p.list().to_vec());
        q.load(&p.values());
        q.load(&[("unknown".into(), 3.0)]);
        assert_eq!(q.values(), p.values());
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(37), "C#2");
    }
}
