// SPDX-License-Identifier: GPL-3.0-or-later
//! Track and effect chain presets (plan A3): a whole track's set-up (its
//! instrument, layers, effects, amp, sends and mixer settings) or just
//! its effect chain, kept to add to any project.
//!
//! A preset holds a [`Track`] without anything that belongs to one song:
//! no clips, notes, takes, comps, automation or frozen audio. What the
//! track pointed at in its own project (the buses its sends and output
//! go to, the track a sidechain listens to) is kept by name, as ids mean
//! nothing in another project; adding the preset finds tracks with those
//! names there, and leaves out what it can't find.
//!
//! On disk a preset is a bundle (see the collab crate) whose project has
//! the one track, with the preset's kind, tags and description in a
//! short text beside the document ([`Preset::meta`]). So the sound files
//! it uses (a cabinet's response, an amp capture, a sampler's sounds)
//! travel with it, and a teacher can send a tone as one file.

use std::collections::{HashMap, HashSet};

use crate::{Effect, Project, Track, TrackKind, new_id};

/// What preset files are called: `Crunch guitar.dawpreset`.
pub const PRESET_EXTENSION: &str = "dawpreset";

/// The most tags a preset keeps, and the longest a tag may be.
pub const MAX_TAGS: usize = 16;
pub const MAX_TAG: usize = 32;

/// What a preset holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PresetKind {
    /// The whole track: instrument, layers, effects, sends, output and
    /// mixer settings.
    #[default]
    Track,
    /// Only the track's own effect chain (its containers included, its
    /// layers' effects not).
    Chain,
}

impl PresetKind {
    /// As the preset file stores it. Never change one.
    pub fn id(self) -> &'static str {
        match self {
            PresetKind::Track => "track",
            PresetKind::Chain => "chain",
        }
    }

    pub fn from_id(id: &str) -> Option<PresetKind> {
        [PresetKind::Track, PresetKind::Chain]
            .into_iter()
            .find(|k| k.id() == id)
    }

    /// What the user sees.
    pub fn name(self) -> &'static str {
        match self {
            PresetKind::Track => "Track",
            PresetKind::Chain => "Effect chain",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub name: String,
    pub kind: PresetKind,
    /// Words to find it by ("crunch", "lesson 3"), lower case, each once.
    pub tags: Vec<String>,
    /// A line or two on what it is for.
    pub about: String,
    /// The track it was saved from, without its song (see the module
    /// notes). Buses and sidechains are named by the tracks' names.
    pub track: Track,
}

/// What adding a preset couldn't keep.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dropped {
    /// Tracks the preset sends, outputs or listens to that this project
    /// has none of by that name.
    pub missing: Vec<String>,
}

/// Tags as typed, comma separated: trimmed, lower case, each once, at
/// most [`MAX_TAGS`] of at most [`MAX_TAG`] characters.
pub fn parse_tags(text: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for t in text.split(',') {
        let t: String = t.trim().to_lowercase().chars().take(MAX_TAG).collect();
        let t = t.trim().to_string();
        if !t.is_empty() && !tags.contains(&t) && tags.len() < MAX_TAGS {
            tags.push(t);
        }
    }
    tags
}

/// The effects in `effects` that belong to one of `layers` (and those
/// nested inside them), by index.
fn layered(effects: &[Effect], layers: &HashSet<&str>) -> HashSet<usize> {
    let mut inside: HashSet<String> = layers.iter().map(|s| s.to_string()).collect();
    let mut found = HashSet::new();
    loop {
        let before = found.len();
        for (i, e) in effects.iter().enumerate() {
            if !found.contains(&i) && e.container.as_ref().is_some_and(|c| inside.contains(c)) {
                found.insert(i);
                inside.insert(e.id.clone());
            }
        }
        if found.len() == before {
            return found;
        }
    }
}

/// Calls `f` on every other track `t` names: its output, its sends' and
/// instrument outputs' buses, and its effects' sidechains.
fn each_route(t: &mut Track, mut f: impl FnMut(&mut String)) {
    for r in (t.output.iter_mut())
        .chain(t.sends.iter_mut().map(|s| &mut s.to))
        .chain(t.instrument_outputs.iter_mut().map(|o| &mut o.to))
        .chain(t.effects.iter_mut().filter_map(|e| e.sidechain.as_mut()))
    {
        f(r);
    }
}

/// Calls `f` on every file `e` uses, and on those of the effects inside
/// it, putting back what `f` returns (or leaving it, for `None`).
fn rename_in(e: &mut Effect, f: &mut impl FnMut(&str) -> Option<String>) {
    if e.asset().is_some()
        && let Some(n) = f(&e.response)
    {
        e.response = n;
    }
    for (_, r) in &mut e.samples {
        if !r.is_empty()
            && !r.starts_with("made:")
            && let Some(n) = f(r)
        {
            *r = n;
        }
    }
    for i in &mut e.inner {
        rename_in(i, f);
    }
}

impl Preset {
    /// A preset of `kind` from track `ti` of `p`.
    pub fn of_track(p: &Project, ti: usize, kind: PresetKind, name: &str) -> Option<Preset> {
        let src = p.tracks.get(ti)?;
        let name_of = |id: &str| {
            (p.tracks.iter())
                .find(|t| t.id == id)
                .map(|t| t.name.clone())
        };
        let mut t = Track::new(String::new(), src.name.clone());
        t.effects = src.effects.clone();
        match kind {
            PresetKind::Chain => {
                let layers: HashSet<&str> = src.layers.iter().map(|l| l.id.as_str()).collect();
                let gone = layered(&t.effects, &layers);
                t.effects = (t.effects.into_iter().enumerate())
                    .filter(|(i, _)| !gone.contains(i))
                    .map(|(_, e)| e)
                    .collect();
            }
            PresetKind::Track => {
                t.gain_db = src.gain_db;
                t.pan = src.pan;
                t.kind = src.kind;
                t.instrument = src.instrument.clone();
                t.layers = src.layers.clone();
                t.notation = src.notation.clone();
                t.output = src.output.as_deref().and_then(name_of);
                t.sends = (src.sends.iter())
                    .filter_map(|s| {
                        let mut s = s.clone();
                        s.to = name_of(&s.to)?;
                        Some(s)
                    })
                    .collect();
                t.instrument_outputs = (src.instrument_outputs.iter())
                    .filter_map(|o| {
                        let mut o = o.clone();
                        o.to = name_of(&o.to)?;
                        Some(o)
                    })
                    .collect();
            }
        }
        for e in &mut t.effects {
            e.sidechain = e.sidechain.as_deref().and_then(name_of);
        }
        Some(Preset {
            name: name.to_string(),
            kind,
            tags: Vec::new(),
            about: String::new(),
            track: t,
        })
    }

    /// Every file the preset uses: asset names or full paths, each once.
    pub fn files(&self) -> Vec<String> {
        let t = &self.track;
        let mut v: Vec<String> = (t.instrument.iter())
            .chain(t.layers.iter().map(|l| &l.instrument))
            .chain(&t.effects)
            .flat_map(|e| e.assets())
            .map(str::to_string)
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// Renames the files it uses: each to what `f` returns for it, or
    /// left as it is for `None`.
    pub fn rename_files(&mut self, mut f: impl FnMut(&str) -> Option<String>) {
        let t = &mut self.track;
        for e in (t.instrument.iter_mut())
            .chain(t.layers.iter_mut().map(|l| &mut l.instrument))
            .chain(&mut t.effects)
        {
            rename_in(e, &mut f);
        }
    }

    /// The preset as a project to write as a document: its track first,
    /// then a stand-in bus for each track it names, as the document
    /// keeps only routes to tracks it has.
    pub fn to_project(&self) -> Project {
        let mut t = self.track.clone();
        let mut named: Vec<Track> = Vec::new();
        each_route(&mut t, |name| {
            let id = match named.iter().find(|n| n.name == *name) {
                Some(n) => n.id.clone(),
                None => {
                    let mut n = Track::new(format!("named-{}", named.len()), name.clone());
                    n.kind = TrackKind::Bus;
                    let id = n.id.clone();
                    named.push(n);
                    id
                }
            };
            *name = id;
        });
        let mut tracks = vec![t];
        tracks.extend(named);
        Project {
            tracks,
            ..Project::default()
        }
    }

    /// The preset from a document's project and its [`meta`] text, or
    /// why it isn't one.
    ///
    /// [`meta`]: Preset::meta
    pub fn from_parts(p: Project, meta: &str, name: &str) -> Result<Preset, String> {
        let mut preset = Preset {
            name: name.to_string(),
            kind: PresetKind::Track,
            tags: Vec::new(),
            about: String::new(),
            track: Track::new(String::new(), String::new()),
        };
        let mut kind = None;
        for line in meta.lines() {
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            let v = v.trim();
            match k.trim() {
                "kind" => kind = PresetKind::from_id(v),
                "name" if !v.is_empty() => preset.name = v.to_string(),
                "tags" => preset.tags = parse_tags(v),
                // Kept on one line: line breaks were written as "\n".
                "about" => preset.about = v.replace("\\n", "\n"),
                _ => {}
            }
        }
        preset.kind = kind.ok_or("it doesn't say what kind of preset it is")?;
        let mut tracks = p.tracks.into_iter();
        preset.track = tracks.next().ok_or("it holds no track")?;
        // Routes go back to the names of the stand-ins they go to.
        let names: HashMap<String, String> = tracks.map(|t| (t.id, t.name)).collect();
        each_route(&mut preset.track, |r| {
            if let Some(n) = names.get(r.as_str()) {
                *r = n.clone();
            }
        });
        preset.track.clips.clear();
        preset.track.note_clips.clear();
        preset.track.frozen = None;
        Ok(preset)
    }

    /// The preset's name, kind, tags and description, as the text kept
    /// beside its document.
    pub fn meta(&self) -> String {
        let one_line = |s: &str| s.replace('\r', "").replace('\n', "\\n");
        format!(
            "kind: {}\nname: {}\ntags: {}\nabout: {}\n",
            self.kind.id(),
            one_line(&self.name),
            self.tags.join(", "),
            one_line(&self.about),
        )
    }

    /// The preset's track for project `p`: with new ids for it and
    /// everything in it, and its buses and sidechains found by name.
    /// Says what it couldn't find.
    pub fn track_for(&self, p: &Project) -> (Track, Dropped) {
        let mut t = self.track.clone();
        t.id = new_id();
        // Every id in it gets a new one; containers follow.
        let mut ids: HashMap<String, String> = HashMap::new();
        let mut renew = |id: &mut String| {
            let new = new_id();
            ids.insert(std::mem::replace(id, new.clone()), new);
        };
        for e in &mut t.effects {
            renew(&mut e.id);
        }
        for l in &mut t.layers {
            renew(&mut l.id);
            renew(&mut l.instrument.id);
        }
        if let Some(i) = &mut t.instrument {
            renew(&mut i.id);
        }
        for e in &mut t.effects {
            e.container = e.container.as_ref().and_then(|c| ids.get(c).cloned());
        }
        let mut dropped = Dropped::default();
        let mut missing = |name: &str| {
            if !dropped.missing.iter().any(|m| m == name) {
                dropped.missing.push(name.to_string());
            }
        };
        let bus = |name: &str| {
            (p.tracks.iter())
                .find(|b| matches!(b.kind, TrackKind::Bus | TrackKind::Folder) && b.name == name)
                .map(|b| b.id.clone())
        };
        if let Some(o) = t.output.take() {
            match bus(&o) {
                Some(id) => t.output = Some(id),
                None => missing(&o),
            }
        }
        t.sends.retain_mut(|s| match bus(&s.to) {
            Some(id) => {
                s.to = id;
                true
            }
            None => {
                missing(&s.to);
                false
            }
        });
        t.instrument_outputs.retain_mut(|o| match bus(&o.to) {
            Some(id) => {
                o.to = id;
                true
            }
            None => {
                missing(&o.to);
                false
            }
        });
        for e in &mut t.effects {
            if let Some(name) = e.sidechain.take() {
                match p.tracks.iter().find(|s| s.name == name) {
                    Some(s) => e.sidechain = Some(s.id.clone()),
                    None => missing(&name),
                }
            }
        }
        t.parent = None;
        t.group = None;
        (t, dropped)
    }

    /// Puts the preset onto track `ti` of `p`: a chain preset replaces
    /// the track's own effect chain (its layers keep theirs); a track
    /// preset replaces its instrument, layers, effects, sends, output and
    /// mixer settings, keeping its name, clips, notes and place. A bus or
    /// folder's preset only goes onto a track of that kind, and another
    /// onto an audio or instrument track. Says what it couldn't find.
    pub fn apply_to(&self, p: &mut Project, ti: usize) -> Result<Dropped, String> {
        let (new, dropped) = self.track_for(p);
        let t = p.tracks.get_mut(ti).ok_or("There is no such track.")?;
        let plays = |k: TrackKind| matches!(k, TrackKind::Audio | TrackKind::Instrument);
        match self.kind {
            PresetKind::Chain => {
                let layers: HashSet<&str> = t.layers.iter().map(|l| l.id.as_str()).collect();
                let keep = layered(&t.effects, &layers);
                let mut effects: Vec<Effect> = (t.effects.drain(..).enumerate())
                    .filter(|(i, _)| keep.contains(i))
                    .map(|(_, e)| e)
                    .collect();
                effects.extend(new.effects);
                t.effects = effects;
            }
            PresetKind::Track => {
                if plays(new.kind) != plays(t.kind) || (!plays(t.kind) && new.kind != t.kind) {
                    return Err(format!(
                        "This preset is for {} track; add it as a new track instead.",
                        match new.kind {
                            TrackKind::Bus => "a bus",
                            TrackKind::Folder => "a folder",
                            _ => "an audio or instrument",
                        }
                    ));
                }
                t.kind = new.kind;
                t.gain_db = new.gain_db;
                t.pan = new.pan;
                t.instrument = new.instrument;
                t.layers = new.layers;
                t.effects = new.effects;
                t.instrument_outputs = new.instrument_outputs;
                t.sends = new.sends;
                t.output = new.output;
                t.notation = new.notation;
                t.freeze = false;
                t.frozen = None;
            }
        }
        Ok(dropped)
    }

    /// Adds the preset to `p` as a new track at the end, named after it
    /// for a chain preset (which makes an audio track) or as saved for a
    /// track preset. Returns where, and what it couldn't find.
    pub fn add_to(&self, p: &mut Project) -> (usize, Dropped) {
        let (mut t, dropped) = self.track_for(p);
        if self.kind == PresetKind::Chain {
            t.name = self.name.clone();
            t.kind = TrackKind::Audio;
        }
        p.tracks.push(t);
        (p.tracks.len() - 1, dropped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Clip, Layer, ProjectDoc, Send, new_peer};

    fn fx(id: &str, builtin: &str) -> Effect {
        Effect {
            id: id.into(),
            name: builtin.into(),
            builtin: builtin.into(),
            params: vec![("gain".into(), 7.0)],
            ..Effect::default()
        }
    }

    fn project() -> Project {
        let mut guitar = Track::new("g".into(), "Guitar".into());
        guitar.gain_db = -3.0;
        guitar.pan = 0.25;
        guitar.mute = true;
        guitar.clips.push(Clip {
            id: "c".into(),
            asset: "0123456789abcdef0123456789abcdef.wav".into(),
            ..Clip::default()
        });
        let mut container = fx("par", "parallel");
        container.params = vec![("chains".into(), 2.0)];
        let mut amp = fx("amp", "amp");
        amp.container = Some("par".into());
        amp.branch = 1;
        let mut cab = fx("cab", "cabinet");
        cab.response = "/home/me/cab.wav".into();
        let mut comp = fx("comp", "compressor");
        comp.sidechain = Some("k".into());
        guitar.effects = vec![container, amp, cab, comp];
        guitar.sends = vec![Send::to("verb".into()), Send::to("gone".into())];
        guitar.output = Some("bus".into());
        guitar.automation.push(crate::Lane::default());
        let mut verb = Track::new("verb".into(), "Reverb".into());
        verb.kind = TrackKind::Bus;
        let mut bus = Track::new("bus".into(), "Guitars".into());
        bus.kind = TrackKind::Bus;
        let kick = Track::new("k".into(), "Kick".into());
        Project {
            tracks: vec![guitar, verb, bus, kick],
            ..Project::default()
        }
    }

    #[test]
    fn a_track_preset_keeps_its_settings_but_not_its_song() {
        let p = project();
        let pr = Preset::of_track(&p, 0, PresetKind::Track, "Crunch").unwrap();
        let t = &pr.track;
        assert!(t.clips.is_empty() && t.automation.is_empty());
        assert_eq!((t.gain_db, t.pan, t.mute), (-3.0, 0.25, false));
        assert_eq!(t.effects.len(), 4);
        assert_eq!(t.output.as_deref(), Some("Guitars"));
        let sends: Vec<_> = t.sends.iter().map(|s| s.to.as_str()).collect();
        assert_eq!(sends, ["Reverb"], "a send to nothing is left out");
        assert_eq!(t.effects[3].sidechain.as_deref(), Some("Kick"));
        assert_eq!(pr.files(), ["/home/me/cab.wav"]);
    }

    #[test]
    fn a_chain_preset_leaves_out_the_layers_and_their_effects() {
        let mut p = project();
        let t = &mut p.tracks[0];
        t.kind = TrackKind::Instrument;
        t.layers.push(Layer::new("l1".into(), fx("syn", "synth")));
        let mut on_layer = fx("lf", "eq");
        on_layer.container = Some("l1".into());
        let mut nested = fx("nest", "delay");
        nested.container = Some("lf".into());
        t.effects.push(on_layer);
        t.effects.push(nested);
        let pr = Preset::of_track(&p, 0, PresetKind::Chain, "Chain").unwrap();
        let ids: Vec<_> = pr.track.effects.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["par", "amp", "cab", "comp"]);
        assert!(pr.track.layers.is_empty() && pr.track.sends.is_empty());

        // Put onto the track, the layer keeps its effects.
        let before = p.tracks[0].effects.len();
        pr.apply_to(&mut p, 0).unwrap();
        let t = &p.tracks[0];
        assert_eq!(t.effects.len(), before - 4 + 4);
        assert!(t.effects.iter().any(|e| e.id == "lf"));
        assert!(t.effects.iter().all(|e| e.id != "amp"), "new ids");
    }

    #[test]
    fn added_to_another_project_it_finds_buses_by_name_and_gets_new_ids() {
        let p = project();
        let pr = Preset::of_track(&p, 0, PresetKind::Track, "Crunch").unwrap();
        let mut other = Project::default();
        let mut verb = Track::new("v2".into(), "Reverb".into());
        verb.kind = TrackKind::Bus;
        other.tracks.push(verb);
        let (i, dropped) = pr.add_to(&mut other);
        let t = &other.tracks[i];
        assert_eq!(t.name, "Guitar");
        assert_eq!(t.sends[0].to, "v2");
        assert_eq!(t.output, None);
        assert_eq!(t.effects[3].sidechain, None);
        assert_eq!(dropped.missing, ["Guitars", "Kick"]);
        // The amp is still in the container, under its new id.
        let par = &t.effects[0];
        assert_ne!(par.id, "par");
        assert_eq!(t.effects[1].container.as_ref(), Some(&par.id));
        assert_eq!(t.effects[1].branch, 1);
    }

    #[test]
    fn a_bus_preset_only_goes_onto_a_bus() {
        let p = project();
        let pr = Preset::of_track(&p, 1, PresetKind::Track, "Verb bus").unwrap();
        let mut q = project();
        assert!(pr.apply_to(&mut q, 0).is_err());
        assert!(pr.apply_to(&mut q, 2).is_ok());
    }

    #[test]
    fn it_goes_through_a_document_and_its_text() {
        let p = project();
        let mut pr = Preset::of_track(&p, 0, PresetKind::Chain, "Crunch").unwrap();
        pr.tags = parse_tags(" Guitar, crunch,guitar,, Lesson 3 ");
        assert_eq!(pr.tags, ["guitar", "crunch", "lesson 3"]);
        pr.about = "For the riff.\nTurn the gain down to clean up.".into();
        let mut doc = ProjectDoc::new(new_peer());
        doc.set_quietly(&pr.to_project());
        let back = ProjectDoc::from_bytes(&doc.to_bytes(), new_peer())
            .unwrap()
            .get();
        let read = Preset::from_parts(back, &pr.meta(), "file name").unwrap();
        assert_eq!(read, pr);
        assert!(Preset::from_parts(Project::default(), &pr.meta(), "x").is_err());

        // A track preset's routes come back by name.
        let pr = Preset::of_track(&p, 0, PresetKind::Track, "Crunch").unwrap();
        doc.set_quietly(&pr.to_project());
        let back = ProjectDoc::from_bytes(&doc.to_bytes(), new_peer())
            .unwrap()
            .get();
        let read = Preset::from_parts(back, &pr.meta(), "file name").unwrap();
        assert_eq!(read, pr);
        assert!(Preset::from_parts(pr.to_project(), "tags: a", "x").is_err());
    }

    #[test]
    fn files_are_renamed_wherever_they_are() {
        let p = project();
        let mut pr = Preset::of_track(&p, 0, PresetKind::Track, "Crunch").unwrap();
        pr.rename_files(|f| Some(format!("{f}!")));
        assert_eq!(pr.files(), ["/home/me/cab.wav!"]);
    }
}
