// SPDX-License-Identifier: GPL-3.0-or-later
//! Instruments that come with the app (roadmap 3.4): a synth of our own
//! design, a multi-zone sampler, a drum sampler, a drum machine with
//! layered pads, and a slicer that turns a loop into a playable
//! instrument.
//!
//! A [`Builtin`] holds an instrument's settings and sounds off the
//! audio thread; [`Builtin::make`] makes the instrument that plays on
//! it, sharing the settings without locks, so moving a control is heard
//! at once. Changing what it is made of (a sound, a zone, a pad's
//! effects) needs a new instrument, made the same way.
//!
//! Nothing here is sampled from another product: the sounds made here
//! ([`made`]) come from code, and every method follows a published one,
//! noted where it is used.

use std::path::Path;
use std::sync::Arc;

use daw_engine::graph::Insert;
use daw_engine::midi::Instrument;

pub mod drum_machine;
pub mod drum_sampler;
pub mod drum_voice;
pub mod dsp;
pub mod made;
pub mod params;
pub mod sample;
pub mod sampler;
pub mod slicer;
pub mod synth;

pub use params::{Param, Params, Unit};
pub use sample::{Loaded, Sample};

/// Which instrument.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Synth,
    Sampler,
    DrumSampler,
    DrumMachine,
    Slicer,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::Synth,
        Kind::Sampler,
        Kind::DrumSampler,
        Kind::DrumMachine,
        Kind::Slicer,
    ];

    /// How projects name it. Never change one.
    pub fn id(self) -> &'static str {
        match self {
            Kind::Synth => "poly-synth",
            Kind::Sampler => "sampler",
            Kind::DrumSampler => "drum-sampler",
            Kind::DrumMachine => "drum-machine",
            Kind::Slicer => "slicer",
        }
    }

    pub fn from_id(id: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.id() == id)
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Synth => "Synth",
            Kind::Sampler => "Sampler",
            Kind::DrumSampler => "Drum sampler",
            Kind::DrumMachine => "Drum machine",
            Kind::Slicer => "Loop slicer",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Kind::Synth => {
                "Two oscillators, a sub, noise, unison, a filter with its own envelope and an LFO"
            }
            Kind::Sampler => "Sound files spread over the keys and over how hard you play",
            Kind::DrumSampler => "One sound file played as a hit, tuned, trimmed and filtered",
            Kind::DrumMachine => {
                "Sixteen pads of layered samples and drums made here, with choke groups, effects, outputs and sends"
            }
            Kind::Slicer => "Cuts a loop at each hit and puts the slices on the keys",
        }
    }
}

/// An effect on one of a drum machine's pads.
pub struct PadFx {
    /// Its id in the project.
    pub id: String,
    pub pad: usize,
    pub effect: Arc<daw_effects::Effect>,
    pub bypass: bool,
}

/// An instrument's settings and sounds.
pub struct Builtin {
    kind: Kind,
    params: Arc<Params>,
    /// Its sounds by slot: `sample` (drum sampler), `loop` (slicer),
    /// `zone0`… (sampler), `pad0.layer0`… (drum machine).
    samples: Vec<(String, Arc<Loaded>)>,
    /// A drum machine's pads' effects, each pad's in order.
    pub fx: Vec<PadFx>,
}

/// Reads a sound file into memory.
pub fn read_file(path: &Path) -> Result<Sample, String> {
    let s = daw_engine::import::read(path)?;
    if s.samples.is_empty() {
        return Err(format!("{}: no sound in it", path.display()));
    }
    Ok(Sample::interleaved(&s.samples, s.channels, s.rate))
}

/// A sound by reference: one made here, or a file `read` reads.
pub fn load(
    reference: &str,
    read: &dyn Fn(&str) -> Result<Sample, String>,
) -> Result<Loaded, String> {
    let s = match made::sound(reference) {
        Some(s) => s,
        None if reference.starts_with(made::PREFIX) => {
            return Err(format!("{reference}: not a sound this version makes"));
        }
        None => read(reference)?,
    };
    Ok(Loaded::new(reference.to_string(), Some(Arc::new(s))))
}

/// A sound made here, by id.
fn made_sound(id: &str) -> Arc<Loaded> {
    let r = made::reference(id);
    Arc::new(Loaded::new(r.clone(), made::sound(&r).map(Arc::new)))
}

impl Builtin {
    /// The instrument with its default settings and sounds.
    pub fn new(kind: Kind) -> Builtin {
        match kind {
            Kind::Synth => Builtin::of(kind, synth::params(), Vec::new()),
            Kind::Sampler => Builtin::of(
                kind,
                sampler::params(1),
                vec![("zone0".into(), made_sound("keys"))],
            ),
            Kind::DrumSampler => Builtin::of(
                kind,
                drum_sampler::params(),
                vec![("sample".into(), made_sound("kick"))],
            ),
            Kind::DrumMachine => {
                let b = Builtin::of(kind, drum_machine::params(), Vec::new());
                drum_machine::default_kit(&b.params);
                b
            }
            Kind::Slicer => {
                let mut b = Builtin::of(kind, slicer::params(0), Vec::new());
                b.set_loop(made_sound("loop"), 0.5);
                b
            }
        }
    }

    fn of(kind: Kind, list: Vec<Param>, samples: Vec<(String, Arc<Loaded>)>) -> Builtin {
        Builtin {
            kind,
            params: Arc::new(Params::new(list)),
            samples,
            fx: Vec::new(),
        }
    }

    /// The instrument as a project has it: its settings by key, its
    /// sounds by slot (read with `read` unless made here) and a drum
    /// machine's pads' effects. A sound that can't be read keeps its
    /// reference, plays nothing, and is named in the problems returned.
    pub fn restore(
        kind: Kind,
        values: &[(String, f64)],
        samples: &[(String, String)],
        fx: Vec<PadFx>,
        read: &dyn Fn(&str) -> Result<Sample, String>,
    ) -> (Builtin, Vec<String>) {
        let count = |prefix: &str, from_samples: bool| -> usize {
            let n = |k: &str| -> Option<usize> {
                let rest = k.strip_prefix(prefix)?;
                let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
                digits.parse::<usize>().ok().map(|i| i + 1)
            };
            let a = values.iter().filter_map(|(k, _)| n(k)).max().unwrap_or(0);
            let b = if from_samples {
                samples.iter().filter_map(|(k, _)| n(k)).max().unwrap_or(0)
            } else {
                0
            };
            a.max(b)
        };
        let list = match kind {
            Kind::Synth => synth::params(),
            Kind::Sampler => sampler::params(count("zone", true)),
            Kind::DrumSampler => drum_sampler::params(),
            Kind::DrumMachine => drum_machine::params(),
            Kind::Slicer => slicer::params(count("slice", false)),
        };
        let mut problems = Vec::new();
        let loaded = (samples.iter())
            .map(|(k, r)| {
                let l = load(r, read).unwrap_or_else(|e| {
                    problems.push(e);
                    Loaded::new(r.clone(), None)
                });
                (k.clone(), Arc::new(l))
            })
            .collect();
        let b = Builtin {
            kind,
            params: Arc::new(Params::new(list)),
            samples: loaded,
            fx,
        };
        b.params.load(values);
        (b, problems)
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn params(&self) -> &Arc<Params> {
        &self.params
    }

    /// Its sounds by slot, as the project saves them.
    pub fn samples(&self) -> Vec<(String, String)> {
        (self.samples.iter())
            .map(|(k, l)| (k.clone(), l.reference.clone()))
            .collect()
    }

    pub fn sample(&self, slot: &str) -> Option<&Arc<Loaded>> {
        self.samples.iter().find(|(k, _)| k == slot).map(|(_, l)| l)
    }

    /// Puts `sound` in `slot`, or empties it.
    pub fn set_sample(&mut self, slot: &str, sound: Option<Loaded>) {
        self.samples.retain(|(k, _)| k != slot);
        if let Some(s) = sound {
            self.samples.push((slot.to_string(), Arc::new(s)));
        }
    }

    /// Gives a sound file a new reference (once it has come into the
    /// project), keeping what was read from it.
    pub fn rename_sample(&mut self, old: &str, new: &str) {
        for (_, l) in &mut self.samples {
            if l.reference == old {
                *l = Arc::new(Loaded::new(new.to_string(), l.sample.clone()));
            }
        }
    }

    /// New settings in the shape `list` gives, carrying over every value
    /// whose key `rename` maps to one there.
    fn reshape(&mut self, list: Vec<Param>, rename: impl Fn(&str) -> Option<String>) {
        let old = self.params.values();
        let p = Params::new(list);
        for (k, v) in old {
            if let Some(k) = rename(&k) {
                p.set_key(&k, v);
            }
        }
        self.params = Arc::new(p);
    }

    // The sampler's zones.

    pub fn zones(&self) -> usize {
        match self.kind {
            Kind::Sampler => (self.params.len() - sampler::at::ZONES) / sampler::ZONE,
            _ => 0,
        }
    }

    /// Adds a zone for each sound, given with its name: at the root note
    /// its name gives, or else on the keys after the last zone's, and
    /// spreads every zone with a root over the keyboard.
    pub fn add_zones(&mut self, sounds: Vec<(String, Loaded)>) {
        let first = self.zones();
        let n = first + sounds.len();
        self.reshape(sampler::params(n), |k| Some(k.to_string()));
        let mut next = 60u8;
        for z in 0..first {
            let root = self.params.get(zone_at(z, sampler::zone::ROOT)) as u8;
            next = next.max(root.saturating_add(1));
        }
        for (i, (name, s)) in sounds.into_iter().enumerate() {
            let z = first + i;
            let root = sampler::note_in_name(&name).unwrap_or_else(|| {
                let r = next;
                next = next.saturating_add(1).min(127);
                r
            });
            self.params
                .set(zone_at(z, sampler::zone::ROOT), root as f64);
            self.set_sample(&format!("zone{z}"), Some(s));
        }
        self.spread_zones();
    }

    /// Spreads the zones over the keyboard by their root notes.
    pub fn spread_zones(&mut self) {
        let roots: Vec<u8> = (0..self.zones())
            .map(|z| self.params.get(zone_at(z, sampler::zone::ROOT)) as u8)
            .collect();
        for (z, (lo, hi)) in sampler::spread(&roots).into_iter().enumerate() {
            self.params.set(zone_at(z, sampler::zone::LO), lo as f64);
            self.params.set(zone_at(z, sampler::zone::HI), hi as f64);
        }
    }

    pub fn remove_zone(&mut self, zone: usize) {
        let n = self.zones();
        if zone >= n {
            return;
        }
        let renumber = |k: &str| -> Option<String> {
            let Some(rest) = k.strip_prefix("zone") else {
                return Some(k.to_string());
            };
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            let i: usize = digits.parse().ok()?;
            let tail = &rest[digits.len()..];
            match i.cmp(&zone) {
                std::cmp::Ordering::Less => Some(k.to_string()),
                std::cmp::Ordering::Equal => None,
                std::cmp::Ordering::Greater => Some(format!("zone{}{tail}", i - 1)),
            }
        };
        self.reshape(sampler::params(n - 1), renumber);
        self.samples = std::mem::take(&mut self.samples)
            .into_iter()
            .filter_map(|(k, l)| renumber(&k).map(|k| (k, l)))
            .collect();
    }

    // The slicer's loop.

    /// Plays `sound` as the loop, cut where its hits start.
    pub fn set_loop(&mut self, sound: Arc<Loaded>, sensitivity: f32) {
        let beats = sound
            .sample
            .as_ref()
            .map_or(4.0, |s| slicer::guess_beats(s.seconds()));
        self.samples.retain(|(k, _)| k != "loop");
        self.samples.push(("loop".into(), sound));
        self.slice_at_hits(sensitivity);
        self.params.set(slicer::at::BEATS, beats);
    }

    pub fn loop_sample(&self) -> Option<Arc<Sample>> {
        self.sample("loop").and_then(|l| l.sample.clone())
    }

    /// The slicer's slices' starts, in frames of its loop.
    pub fn slices(&self) -> Vec<usize> {
        match self.kind {
            Kind::Slicer => slicer::slices_in(&self.params),
            _ => Vec::new(),
        }
    }

    pub fn slice_at_hits(&mut self, sensitivity: f32) {
        let s = self.loop_sample();
        let slices = s.map_or_else(|| vec![0], |s| slicer::find_hits(&s, sensitivity));
        self.set_slices(slices);
    }

    pub fn slice_equally(&mut self, count: usize) {
        let len = self.loop_sample().map_or(0, |s| s.len());
        self.set_slices(slicer::equal(len, count));
    }

    pub fn set_slices(&mut self, mut slices: Vec<usize>) {
        slices.sort_unstable();
        slices.dedup();
        slices.truncate(slicer::MAX_SLICES);
        self.reshape(slicer::params(slices.len()), |k| {
            (!k.starts_with("slice")).then(|| k.to_string())
        });
        for (i, f) in slices.into_iter().enumerate() {
            self.params.set(slicer::at::SLICES + i, f as f64);
        }
    }

    /// The notes that play the loop's slices in order, as (start,
    /// length, key) in ticks at `ppq` a beat.
    pub fn loop_notes(&self, ppq: i64) -> Vec<(i64, i64, u8)> {
        let len = self.loop_sample().map_or(0, |s| s.len());
        let beats = self.params.get(slicer::at::BEATS);
        slicer::notes(&self.slices(), len, beats, ppq)
    }

    /// Makes the instrument that plays at `rate`, sharing these
    /// settings. Allocates, and may convert sounds to the rate; not for
    /// the audio thread.
    pub fn make(&self, rate: u32) -> Box<dyn Instrument> {
        let at = |slot: &str| self.sample(slot).and_then(|l| l.at(rate));
        let p = self.params.clone();
        match self.kind {
            Kind::Synth => Box::new(synth::Synth::new(rate, p)),
            Kind::Sampler => {
                let zones = (0..self.zones()).map(|z| at(&format!("zone{z}"))).collect();
                Box::new(sampler::Sampler::new(rate, p, zones))
            }
            Kind::DrumSampler => Box::new(drum_sampler::DrumSampler::new(rate, p, at("sample"))),
            Kind::DrumMachine => {
                use drum_machine::{LAYERS, PADS, sample_key};
                let samples = (0..PADS * LAYERS)
                    .map(|i| at(&sample_key(i / LAYERS, i % LAYERS)))
                    .collect();
                let mut fx: Vec<Vec<Box<dyn Insert>>> = (0..PADS).map(|_| Vec::new()).collect();
                for f in self.fx.iter().filter(|f| !f.bypass && f.pad < PADS) {
                    fx[f.pad].push(Box::new(f.effect.processor(rate)));
                }
                Box::new(drum_machine::DrumMachine::new(rate, p, samples, fx))
            }
            Kind::Slicer => {
                let original = self.loop_sample().map_or(rate, |s| s.rate);
                let scale = rate as f64 / original.max(1) as f64;
                Box::new(slicer::Slicer::new(rate, p, at("loop"), scale))
            }
        }
    }
}

/// The index of the sampler's zone `z`'s setting `k`.
pub fn zone_at(z: usize, k: usize) -> usize {
    sampler::at::ZONES + z * sampler::ZONE + k
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::midi::Event;

    fn no_files(r: &str) -> Result<Sample, String> {
        Err(format!("{r}: no files here"))
    }

    fn loudness(b: &Builtin, note: u8) -> f32 {
        let mut i = b.make(44_100);
        let (mut l, mut r) = (vec![0.0; 22_050], vec![0.0; 22_050]);
        i.process(&[Event::note_on(0, 0, note, 110, 0)], &mut l, &mut r);
        l.iter().chain(&r).fold(0.0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn every_instrument_plays_out_of_the_box() {
        for k in Kind::ALL {
            let b = Builtin::new(k);
            let note = match k {
                Kind::DrumMachine | Kind::Slicer => 36,
                _ => 60,
            };
            let peak = loudness(&b, note);
            assert!(peak > 0.02 && peak < 4.0, "{k:?}: {peak}");
            assert_eq!(Kind::from_id(k.id()), Some(k));
        }
    }

    #[test]
    fn an_instrument_comes_back_from_what_a_project_saves() {
        for k in Kind::ALL {
            let b = Builtin::new(k);
            if let Some(first) = b.params.list().first() {
                b.params.set(0, (first.min + first.max) / 2.0);
            }
            let (back, problems) =
                Builtin::restore(k, &b.params.values(), &b.samples(), Vec::new(), &no_files);
            assert!(problems.is_empty(), "{problems:?}");
            assert_eq!(back.params.values(), b.params.values(), "{k:?}");
            assert_eq!(back.samples(), b.samples());
        }
        // A file that can't be read is kept, and named.
        let (b, problems) = Builtin::restore(
            Kind::DrumSampler,
            &[],
            &[("sample".into(), "gone.wav".into())],
            Vec::new(),
            &no_files,
        );
        assert_eq!(problems.len(), 1);
        assert_eq!(
            b.samples(),
            [("sample".to_string(), "gone.wav".to_string())]
        );
    }

    #[test]
    fn zones_are_added_by_their_names_and_removed_keeping_the_rest() {
        let mut b = Builtin::new(Kind::Sampler);
        let tone = |r: &str| {
            Loaded::new(
                r.into(),
                Some(Arc::new(Sample::mono(vec![0.1; 100], 48_000))),
            )
        };
        b.add_zones(vec![
            ("low C3".into(), tone("a.wav")),
            ("high C5".into(), tone("b.wav")),
        ]);
        assert_eq!(b.zones(), 3);
        let root = |b: &Builtin, z| b.params.get(zone_at(z, sampler::zone::ROOT));
        assert_eq!([root(&b, 0), root(&b, 1), root(&b, 2)], [60.0, 48.0, 72.0]);
        assert_eq!(b.params.get(zone_at(1, sampler::zone::LO)), 0.0);
        assert_eq!(b.params.get(zone_at(2, sampler::zone::HI)), 127.0);
        b.params.set(zone_at(2, sampler::zone::TUNE), 33.0);
        b.remove_zone(1);
        assert_eq!(b.zones(), 2);
        assert_eq!(root(&b, 1), 72.0);
        assert_eq!(b.params.get(zone_at(1, sampler::zone::TUNE)), 33.0);
        assert_eq!(b.sample("zone1").unwrap().reference, "b.wav");
        assert!(b.sample("zone2").is_none());
    }

    #[test]
    fn the_loop_made_here_slices_into_its_hits_and_plays_back_in_order() {
        let b = Builtin::new(Kind::Slicer);
        // Eight hi-hats on the eighths, with kicks and snares on some.
        assert_eq!(b.slices().len(), 8, "{:?}", b.slices());
        assert_eq!(b.params.get(slicer::at::BEATS), 4.0);
        let notes = b.loop_notes(960);
        assert_eq!(notes.len(), 8);
        for (i, n) in notes.iter().enumerate() {
            assert!((n.0 - i as i64 * 480).abs() <= 10, "{n:?}");
            assert_eq!(n.2, 36 + i as u8);
        }
        let mut b = b;
        b.slice_equally(4);
        assert_eq!(b.slices(), [0, 24_000, 48_000, 72_000]);
    }
}
