// SPDX-License-Identifier: GPL-3.0-or-later
//! Writing a project as a DAWproject file.
//!
//! The arrangement is written in beats, so notes and markers stay on
//! their beats in the other DAW. Audio clips are placed in beats too,
//! with their content in seconds, as the format's own examples do, so
//! audio plays at its own speed wherever the tempo changes. Automation
//! points are written at the beat their frame falls on, fraction kept.

use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Write};
use std::path::Path;

use daw_engine::time::{TempoMap, Ticks};
use daw_project::{Controller, Effect, Lane, LaneTarget, Note, NoteClip, Project, Track};

use crate::devices::{self, EQ_BANDS, Generic, OWN};
use crate::time::{self, frames_to_beats, frames_to_seconds, num, ticks_to_beats};
use crate::xml::{Xml, b};
use crate::{ExportOptions, Report, VERSION};

/// The file for `p`, and what didn't survive.
pub fn export(p: &Project, o: &ExportOptions) -> Result<(Vec<u8>, Report), String> {
    let mut w = Writer::new(p, o, true);
    let project = w.project();
    let metadata = metadata(o.title);
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let deflate = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    // Audio is stored as it is: it hardly shrinks, and stays quick to read.
    let stored = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .large_file(true);
    let err = |e: &dyn std::fmt::Display| format!("could not write the file: {e}");
    zip.start_file("project.xml", deflate)
        .map_err(|e| err(&e))?;
    zip.write_all(project.as_bytes()).map_err(|e| err(&e))?;
    zip.start_file("metadata.xml", deflate)
        .map_err(|e| err(&e))?;
    zip.write_all(metadata.as_bytes()).map_err(|e| err(&e))?;
    for (path, data) in &w.files {
        let opts = if path.starts_with("audio/") {
            stored
        } else {
            deflate
        };
        zip.start_file(path.as_str(), opts).map_err(|e| err(&e))?;
        zip.write_all(data).map_err(|e| err(&e))?;
    }
    let bytes = zip.finish().map_err(|e| err(&e))?.into_inner();
    Ok((bytes, w.report))
}

/// What an export of `p` would leave out, without writing anything.
pub fn export_report(p: &Project, o: &ExportOptions) -> Report {
    let mut w = Writer::new(p, o, false);
    w.project();
    w.report
}

fn metadata(title: &str) -> String {
    let mut x = Xml::new();
    x.open("MetaData", &[]);
    if !title.is_empty() {
        x.text("Title", title);
    }
    x.close();
    x.finish()
}

/// The ids written for a track's parts, made before anything is
/// written, as tracks name each other's channels.
#[derive(Default)]
struct TrackIds {
    track: String,
    channel: String,
    volume: String,
    pan: String,
    /// Each send's level, by the bus's id.
    sends: HashMap<String, String>,
}

/// An audio file's shape, read from its header.
#[derive(Clone)]
struct AudioInfo {
    path: String,
    channels: u16,
    rate: u32,
    seconds: f64,
}

struct Writer<'a> {
    p: &'a Project,
    o: &'a ExportOptions<'a>,
    map: TempoMap,
    /// Reading the files to put them in, or only looking at them for
    /// the report.
    with_files: bool,
    next: usize,
    ids: HashMap<String, TrackIds>,
    master: String,
    tempo: String,
    signature: String,
    /// Automation targets' ids, by track id and lane.
    params: HashMap<(String, String), String>,
    /// Files to put in the ZIP beside `project.xml`, by path.
    files: Vec<(String, Vec<u8>)>,
    /// Each asset's audio, once read.
    audio: HashMap<String, Option<AudioInfo>>,
    /// Sound files of our own devices, by what the effect names them.
    sounds: HashMap<String, String>,
    taken: HashSet<String>,
    report: Report,
}

impl<'a> Writer<'a> {
    fn new(p: &'a Project, o: &'a ExportOptions<'a>, with_files: bool) -> Writer<'a> {
        Writer {
            p,
            o,
            map: time::tempo_map(p),
            with_files,
            next: 0,
            ids: HashMap::new(),
            master: String::new(),
            tempo: String::new(),
            signature: String::new(),
            params: HashMap::new(),
            files: Vec::new(),
            audio: HashMap::new(),
            sounds: HashMap::new(),
            taken: HashSet::new(),
            report: Report::default(),
        }
    }

    fn id(&mut self) -> String {
        self.next += 1;
        format!("id{}", self.next)
    }

    fn beats(&self, frame: i64) -> f64 {
        frames_to_beats(&self.map, frame)
    }

    fn project(&mut self) -> String {
        self.note_losses();
        self.tempo = self.id();
        self.signature = self.id();
        self.master = self.id();
        for t in &self.p.tracks {
            let mut ids = TrackIds {
                track: String::new(),
                channel: String::new(),
                volume: String::new(),
                pan: String::new(),
                sends: HashMap::new(),
            };
            ids.track = self.id();
            ids.channel = self.id();
            ids.volume = self.id();
            ids.pan = self.id();
            for s in &t.sends {
                let id = self.id();
                ids.sends.insert(s.to.clone(), id);
            }
            self.ids.insert(t.id.clone(), ids);
        }
        let mut x = Xml::new();
        x.open("Project", &[("version", VERSION.into())]);
        x.empty(
            "Application",
            &[
                ("name", "DAW".into()),
                ("version", self.o.app_version.into()),
            ],
        );
        x.open("Transport", &[]);
        x.empty(
            "Tempo",
            &[
                ("max", "999".into()),
                ("min", "10".into()),
                ("unit", "bpm".into()),
                ("value", num(self.p.tempo_bpm)),
                ("id", self.tempo.clone()),
                ("name", "Tempo".into()),
            ],
        );
        let sig = time::signature(self.p.beats_per_bar, self.p.beat_unit);
        x.empty(
            "TimeSignature",
            &[
                ("denominator", sig.unit.to_string()),
                ("numerator", sig.beats.to_string()),
                ("id", self.signature.clone()),
            ],
        );
        x.close();
        self.structure(&mut x);
        self.arrangement(&mut x);
        x.empty("Scenes", &[]);
        x.close();
        x.finish()
    }

    /// Everything the format has no place for, said once each.
    fn note_losses(&mut self) {
        let p = self.p;
        let r = &mut self.report;
        let n = |c: usize, one: &str, many: &str| {
            if c == 1 {
                format!("1 {one}")
            } else {
                format!("{c} {many}")
            }
        };
        if !p.comments.is_empty() {
            let c = n(p.comments.len(), "comment", "comments");
            r.add("Comments", format!("{c} stay in this project only."));
        }
        if !p.notes.trim().is_empty() || !p.tabs.is_empty() {
            r.add(
                "Lesson notes and tab",
                "Stay in this project only; other DAWs have no place for them.",
            );
        }
        if !p.assignments.is_empty() {
            r.add(
                "Practice assignments",
                "Stay in this project only, with the practice log.",
            );
        }
        if !p.edit_groups.is_empty() {
            r.add(
                "Edit groups",
                "Tracks edited together go out as separate tracks.",
            );
        }
        if !p.control_groups.is_empty() {
            r.add(
                "Control groups",
                "The groups' faders and mutes are left out; each track keeps its own.",
            );
        }
        if !p.cue_mixes.is_empty() {
            r.add("Headphone mixes", "Left out; only the main mix goes.");
        }
        if !p.hit_points.is_empty() || p.timecode != Default::default() {
            r.add(
                "Timecode and hit points",
                "Left out; markers go, and audio stays at its time.",
            );
        }
        if p.scale.is_some()
            || p.tracks
                .iter()
                .flat_map(|t| &t.note_clips)
                .any(|c| c.scale.is_some())
        {
            r.add(
                "Key and scale",
                "Left out; the notes themselves go as they are.",
            );
        }
        if !p.grooves.is_empty() {
            r.add("Grooves", "Left out; notes go where they are now.");
        }
        if !p.midi_maps.is_empty() {
            r.add("MIDI learn", "Controls tied to settings are left out.");
        }
        if (p.tracks.iter().flat_map(|t| &t.note_clips)).any(|c| c.pattern.is_some()) {
            r.add(
                "Drum patterns",
                "Pattern clips go as the notes they play now; the pattern itself stays here.",
            );
        }
        if !p.videos.is_empty() {
            r.add(
                "Video",
                "The picture is left out; its sound goes as an audio clip.",
            );
        }
        if p.loop_bars != Project::default().loop_bars {
            r.add("Loop", "The loop range is left out.");
        }
        if p.reference_hz != 440.0 {
            r.add(
                "Tuning reference",
                "Left out; other DAWs tune to A = 440 Hz.",
            );
        }
        for t in &p.tracks {
            let name = format!("Track \"{}\"", t.name);
            if !t.comps.is_empty() || t.clips.iter().any(|c| c.take.is_some() && !plays(t, c)) {
                r.add(
                    &name,
                    "Only the takes that play go; the other takes and comps stay here.",
                );
            }
            if t.frozen.is_some() {
                r.add(
                    &name,
                    "Its frozen audio is left out; its clips and effects go.",
                );
            }
            if t.notation.is_some() {
                r.add(&name, "Its tab or staff lane is left out.");
            }
            if t.layers.len() > 1 {
                r.add(
                    &name,
                    "Only its first layered instrument goes; the other layers are left out.",
                );
            }
            if !t.instrument_outputs.is_empty() {
                r.add(
                    &name,
                    "Its instrument's extra outputs go through the track itself.",
                );
            }
            if t.clips
                .iter()
                .any(|c| c.gain_db != 0.0 || !c.envelope.is_empty())
            {
                r.add(
                    &name,
                    "Clip gain and clip volume lines are left out; the format has no place for them.",
                );
            }
            if t.clips
                .iter()
                .any(|c| c.fade_in.len > 0 || c.fade_out.len > 0)
            {
                r.add(
                    &name,
                    "Fades go with their lengths; their shapes become the other DAW's own.",
                );
            }
            let notes: Vec<&Note> = t.note_clips.iter().flat_map(|c| &c.notes).collect();
            if notes.iter().any(|n| {
                n.string.is_some()
                    || n.fret.is_some()
                    || n.bend != 0.0
                    || !n.techniques.is_empty()
                    || n.articulation.is_some()
            }) {
                r.add(
                    &name,
                    "Strings, frets, bends and playing techniques on notes are left out.",
                );
            }
            if notes
                .iter()
                .any(|n| n.chance < 1.0 || n.group.is_some() || n.one_of)
            {
                r.add(
                    &name,
                    "Notes' chance to play is left out: every note plays.",
                );
            }
            if t.effects.iter().any(|e| e.sidechain.is_some()) {
                r.add(&name, "Sidechain inputs are left out.");
            }
            if t.effects.iter().any(|e| e.container.is_some()) {
                r.add(
                    &name,
                    "Effects inside effect containers are left out; the containers go empty.",
                );
            }
            if t.kind.is_bus() && (!t.clips.is_empty() || !t.note_clips.is_empty()) {
                r.add(&name, "Clips on a bus are left out.");
            }
        }
    }

    fn structure(&mut self, x: &mut Xml) {
        x.open("Structure", &[]);
        let folders: HashSet<&str> = (self.p.tracks.iter())
            .filter(|t| t.kind == daw_project::TrackKind::Folder)
            .map(|t| t.id.as_str())
            .collect();
        for t in &self.p.tracks {
            if t.parent.as_deref().is_none_or(|f| !folders.contains(f)) {
                self.track(x, t);
            }
        }
        x.open(
            "Track",
            &[
                ("contentType", "audio notes".into()),
                ("loaded", b(true)),
                ("id", self.id()),
                ("name", "Master".into()),
            ],
        );
        x.open(
            "Channel",
            &[
                ("audioChannels", "2".into()),
                ("role", "master".into()),
                ("solo", b(false)),
                ("id", self.master.clone()),
            ],
        );
        let (mute, pan, vol) = (self.id(), self.id(), self.id());
        x.empty(
            "Mute",
            &[("value", b(false)), ("id", mute), ("name", "Mute".into())],
        );
        x.empty("Pan", &pan_attrs(0.0, pan));
        x.empty("Volume", &volume_attrs(0.0, vol));
        x.close();
        x.close();
        x.close();
    }

    fn track(&mut self, x: &mut Xml, t: &Track) {
        use daw_project::TrackKind::*;
        let p = self.p;
        let content = match t.kind {
            Audio => "audio",
            Instrument if t.clips.is_empty() => "notes",
            Instrument => "notes audio",
            Bus => "audio",
            Folder => "tracks",
        };
        let role = match t.kind {
            Audio | Instrument => "regular",
            Folder => "submix",
            // A return if any track sends to it, else a group.
            Bus if (p.tracks.iter()).any(|o| o.sends.iter().any(|s| s.to == t.id)) => "effect",
            Bus => "submix",
        };
        let destination = match t.goes_to() {
            Some(to) => self.ids.get(to).map(|i| i.channel.clone()),
            None => None,
        }
        .unwrap_or_else(|| self.master.clone());
        let ids = &self.ids[&t.id];
        let (track_id, channel, volume, pan) = (
            ids.track.clone(),
            ids.channel.clone(),
            ids.volume.clone(),
            ids.pan.clone(),
        );
        x.open(
            "Track",
            &[
                ("contentType", content.into()),
                ("loaded", b(true)),
                ("id", track_id),
                ("name", t.name.clone()),
            ],
        );
        x.open(
            "Channel",
            &[
                ("audioChannels", "2".into()),
                ("destination", destination),
                ("role", role.into()),
                ("solo", b(t.solo)),
                ("id", channel),
            ],
        );
        self.devices(x, t);
        let mute = self.id();
        x.empty(
            "Mute",
            &[("value", b(t.mute)), ("id", mute), ("name", "Mute".into())],
        );
        x.empty("Pan", &pan_attrs(t.pan, pan));
        if !t.sends.is_empty() {
            x.open("Sends", &[]);
            for s in &t.sends {
                let Some(bus) = self.ids.get(&s.to) else {
                    continue;
                };
                let dest = bus.channel.clone();
                let vol = self.ids[&t.id].sends[&s.to].clone();
                let (send, enable) = (self.id(), self.id());
                x.open(
                    "Send",
                    &[
                        ("destination", dest),
                        ("type", if s.pre_fader { "pre" } else { "post" }.into()),
                        ("id", send),
                    ],
                );
                x.empty(
                    "Enable",
                    &[
                        ("value", b(!s.mute)),
                        ("id", enable),
                        ("name", "Enable".into()),
                    ],
                );
                x.empty("Volume", &volume_attrs(s.gain_db, vol));
                x.close();
            }
            x.close();
        }
        x.empty("Volume", &volume_attrs(t.gain_db, volume));
        x.close();
        if t.kind == Folder {
            for c in &p.tracks {
                if c.parent.as_deref() == Some(&t.id) {
                    self.track(x, c);
                }
            }
        }
        x.close();
    }

    fn devices(&mut self, x: &mut Xml, t: &Track) {
        let instrument = t
            .layers
            .first()
            .map(|l| &l.instrument)
            .or(t.instrument.as_ref());
        let effects: Vec<&Effect> = t.effects.iter().filter(|e| e.container.is_none()).collect();
        if instrument.is_none() && effects.is_empty() {
            return;
        }
        x.open("Devices", &[]);
        if let Some(i) = instrument {
            self.device(x, t, i, "instrument");
        }
        for e in effects {
            self.device(x, t, e, "audioFX");
        }
        x.close();
    }

    /// The track's automation lanes on effect `e`, by setting.
    fn lanes_on<'t>(t: &'t Track, e: &Effect) -> Vec<&'t Lane> {
        (t.automation.iter())
            .filter(|l| l.effect() == Some(e.id.as_str()) && !l.points.is_empty())
            .collect()
    }

    fn device(&mut self, x: &mut Xml, t: &Track, e: &Effect, role: &str) {
        let id = self.id();
        let enabled = self.id();
        let lanes = Writer::lanes_on(t, e);
        let track_name = format!("Track \"{}\"", t.name);
        if e.is_plugin() {
            let clap = daw_plugins::clap::is_clap(Path::new(&e.bundle));
            let (element, device_id, ext) = if clap {
                let cid = (self.o.clap_id)(e);
                if cid.is_none() {
                    self.report.add(
                        &track_name,
                        format!(
                            "The plugin \"{}\" couldn't be found here to read its id; other DAWs may not know it.",
                            e.name
                        ),
                    );
                }
                ("ClapPlugin", cid.unwrap_or_default(), "clap-preset")
            } else {
                let hex = devices::class_hex(&e.class).unwrap_or_default();
                ("Vst3Plugin", hex, "vstpreset")
            };
            let state = match &e.state {
                Some(s) => {
                    let path = self.free(&format!("plugins/{}.{ext}", safe(&e.name)));
                    let data = if clap {
                        s.component.clone()
                    } else {
                        daw_plugins::presets::PresetFile {
                            class: device_id.clone(),
                            state: daw_plugins::PluginState {
                                component: s.component.clone(),
                                controller: s.controller.clone(),
                            },
                        }
                        .encode()
                    };
                    self.files.push((path.clone(), data));
                    Some(path)
                }
                None => {
                    self.report.add(
                        &track_name,
                        format!(
                            "The plugin \"{}\" goes without its settings: they weren't saved.",
                            e.name
                        ),
                    );
                    None
                }
            };
            x.open(
                element,
                &[
                    ("deviceID", device_id),
                    ("deviceName", e.name.clone()),
                    ("deviceRole", role.into()),
                    ("loaded", b(true)),
                    ("id", id),
                    ("name", e.name.clone()),
                ],
            );
            if !lanes.is_empty() {
                x.open("Parameters", &[]);
                for l in &lanes {
                    if let LaneTarget::Plugin { id: param, .. } = l.target() {
                        let pid = self.id();
                        self.params
                            .insert((t.id.clone(), l.param.clone()), pid.clone());
                        x.empty(
                            "RealParameter",
                            &[
                                ("max", "1".into()),
                                ("min", "0".into()),
                                ("unit", "normalized".into()),
                                ("parameterID", param.to_string()),
                                ("id", pid),
                            ],
                        );
                    }
                }
                x.close();
            }
            x.empty(
                "Enabled",
                &[
                    ("value", b(!e.bypass)),
                    ("id", enabled),
                    ("name", "On/Off".into()),
                ],
            );
            if let Some(path) = state {
                x.empty("State", &[("path", path)]);
            }
            x.close();
            return;
        }
        // One of ours: as a generic device if it is one, and with our
        // own state either way.
        let generic = Generic::of(&e.builtin);
        if generic.is_none() {
            let kind = daw_effects::Kind::from_id(&e.builtin)
                .map_or(e.name.clone(), |k| k.name().to_string());
            self.report.add(
                &track_name,
                format!(
                    "\"{}\" ({kind}) goes as a device of this app's own: it comes back whole here, but other DAWs can't play it.",
                    e.name
                ),
            );
        }
        if let Some(g) = generic {
            for key in g.unmapped() {
                if !devices::is_default(e, key) {
                    self.report.add(
                        &track_name,
                        format!(
                            "\"{}\": its {key} setting has no place in the generic {}; it comes back here only.",
                            e.name,
                            g.builtin()
                        ),
                    );
                }
            }
        }
        let state = self.own_state(e);
        let element = generic.map_or("BuiltinDevice", Generic::element);
        x.open(
            element,
            &[
                ("deviceID", format!("{OWN}{}", e.builtin)),
                ("deviceName", e.name.clone()),
                ("deviceRole", role.into()),
                ("loaded", b(true)),
                ("id", id),
                ("name", e.name.clone()),
            ],
        );
        // Settings automated on a device of our own are listed so lanes
        // can name them; a generic device's are its own elements.
        if generic.is_none() && !lanes.is_empty() {
            x.open("Parameters", &[]);
            for l in &lanes {
                if let LaneTarget::Fx { key, .. } = l.target() {
                    let pid = self.id();
                    self.params
                        .insert((t.id.clone(), l.param.clone()), pid.clone());
                    x.empty(
                        "RealParameter",
                        &[
                            ("unit", devices::unit_of(&e.builtin, key).into()),
                            ("value", num(devices::value(e, key))),
                            ("id", pid),
                            ("name", key.into()),
                        ],
                    );
                }
            }
            x.close();
        }
        x.empty(
            "Enabled",
            &[
                ("value", b(!e.bypass)),
                ("id", enabled),
                ("name", "On/Off".into()),
            ],
        );
        x.empty("State", &[("path", state)]);
        if let Some(g) = generic {
            self.generic(x, t, e, g);
        }
        x.close();
    }

    /// A generic device's own settings, after its state.
    fn generic(&mut self, x: &mut Xml, t: &Track, e: &Effect, g: Generic) {
        let param = |w: &mut Writer, x: &mut Xml, element: &str, key: &str, unit: &str, v: f64| {
            let pid = w.id();
            w.params.insert(
                (t.id.clone(), format!("{}{}:{key}", Lane::FX, e.id)),
                pid.clone(),
            );
            x.empty(
                element,
                &[
                    ("unit", unit.into()),
                    ("value", num(v)),
                    ("id", pid),
                    ("name", key.into()),
                ],
            );
        };
        if g == Generic::Eq {
            for (order, band) in EQ_BANDS.iter().enumerate() {
                let freq = devices::value(e, band.freq);
                let on = band.off_at.is_none_or(|off| freq != off);
                x.open(
                    "Band",
                    &[("type", band.kind.into()), ("order", order.to_string())],
                );
                param(self, x, "Freq", band.freq, "hertz", freq);
                if let Some(k) = band.gain {
                    param(self, x, "Gain", k, "decibel", devices::value(e, k));
                }
                if let Some(k) = band.q {
                    param(self, x, "Q", k, "linear", devices::value(e, k));
                }
                let en = self.id();
                x.empty(
                    "Enabled",
                    &[("value", b(on)), ("id", en), ("name", "Enabled".into())],
                );
                x.close();
            }
        }
        for gp in g.params() {
            let v = (gp.to)(devices::value(e, gp.key));
            param(self, x, gp.element, gp.key, gp.unit, v);
        }
    }

    /// Writes an effect's own state, and the sound files it uses, and
    /// returns the state file's path.
    fn own_state(&mut self, e: &Effect) -> String {
        let mut copy = e.clone();
        let mut names: Vec<String> = Vec::new();
        devices::map_files(&mut copy, &mut |r| {
            names.push(r.to_string());
            r.to_string()
        });
        for r in names {
            if self.sounds.contains_key(&r) {
                continue;
            }
            let path = (self.o.asset_path)(&r);
            let base = Path::new(&r)
                .file_name()
                .map_or("sound".into(), |n| n.to_string_lossy().into_owned());
            let zp = self.free(&format!("samples/{}", safe(&base)));
            if self.with_files {
                match std::fs::read(&path) {
                    Ok(d) => self.files.push((zp.clone(), d)),
                    Err(_) => {
                        self.report.add(
                            "Sound files",
                            format!("{base} couldn't be read, so it is left out."),
                        );
                        continue;
                    }
                }
            }
            self.sounds.insert(r, zp);
        }
        let sounds = self.sounds.clone();
        devices::map_files(&mut copy, &mut |r| {
            sounds.get(r).map_or(r.to_string(), |z| format!("zip:{z}"))
        });
        let path = self.free(&format!("plugins/{}.dawdevice", safe(&e.name)));
        self.files.push((path.clone(), devices::own_state(&copy)));
        path
    }

    /// `path`, or with a number added if another file has it.
    fn free(&mut self, path: &str) -> String {
        let (stem, ext) = match path.rsplit_once('.') {
            Some((s, x)) if !s.ends_with('/') => (s.to_string(), format!(".{x}")),
            _ => (path.to_string(), String::new()),
        };
        let mut p = path.to_string();
        let mut n = 2;
        while !self.taken.insert(p.to_lowercase()) {
            p = format!("{stem} {n}{ext}");
            n += 1;
        }
        p
    }

    fn arrangement(&mut self, x: &mut Xml) {
        let id = self.id();
        x.open("Arrangement", &[("id", id)]);
        let lanes = self.id();
        x.open("Lanes", &[("timeUnit", "beats".into()), ("id", lanes)]);
        for t in &self.p.tracks {
            let clips: Vec<&daw_project::Clip> = if t.kind.is_bus() {
                Vec::new()
            } else {
                t.clips.iter().filter(|c| plays(t, c)).collect()
            };
            let auto: Vec<&Lane> = (t.automation.iter())
                .filter(|l| !l.points.is_empty())
                .collect();
            if clips.is_empty() && t.note_clips.is_empty() && auto.is_empty() {
                continue;
            }
            let (track, lid) = (self.ids[&t.id].track.clone(), self.id());
            x.open("Lanes", &[("track", track), ("id", lid)]);
            if !clips.is_empty() || !t.note_clips.is_empty() {
                // In time order, audio and notes together.
                enum Any<'c> {
                    Audio(&'c daw_project::Clip),
                    Notes(&'c NoteClip),
                }
                let mut all: Vec<(f64, Any)> = Vec::new();
                for c in clips {
                    all.push((self.beats(c.start), Any::Audio(c)));
                }
                for c in &t.note_clips {
                    all.push((ticks_to_beats(c.start), Any::Notes(c)));
                }
                all.sort_by(|a, b| a.0.total_cmp(&b.0));
                let cid = self.id();
                x.open("Clips", &[("id", cid)]);
                for (_, c) in all {
                    match c {
                        Any::Audio(c) => self.audio_clip(x, t, c),
                        Any::Notes(c) => self.note_clip(x, c),
                    }
                }
                x.close();
            }
            for l in auto {
                self.automation(x, t, l);
            }
            x.close();
        }
        x.close();
        if !self.p.markers.is_empty() {
            let id = self.id();
            x.open("Markers", &[("id", id)]);
            let mut markers: Vec<_> = self.p.markers.iter().collect();
            markers.sort_by_key(|m| m.bar);
            for m in markers {
                let at = self.map.bar_start(m.bar.max(1));
                x.empty(
                    "Marker",
                    &[
                        ("time", num(ticks_to_beats(at.0))),
                        ("name", m.name.clone()),
                    ],
                );
            }
            x.close();
        }
        self.tempo_automation(x);
        x.close();
    }

    fn tempo_automation(&mut self, x: &mut Xml) {
        let changes: Vec<(Ticks, f64, bool)> = (self.map.tempo_changes())
            .map(|(at, t, glide)| (at, t.as_f64(), glide))
            .collect();
        if !changes.is_empty() {
            let id = self.id();
            x.open("TempoAutomation", &[("unit", "bpm".into()), ("id", id)]);
            x.empty("Target", &[("parameter", self.tempo.clone())]);
            // Each point's line runs to the next: straight into a glide,
            // held before a jump.
            let mut points = vec![(Ticks(0), self.map.tempo_at(Ticks(0)).as_f64())];
            points.extend(changes.iter().map(|(at, bpm, _)| (*at, *bpm)));
            for (i, (at, bpm)) in points.iter().enumerate() {
                let glides = changes.get(i).is_some_and(|c| c.2);
                x.empty(
                    "RealPoint",
                    &[
                        ("time", num(ticks_to_beats(at.0))),
                        ("value", num(*bpm)),
                        (
                            "interpolation",
                            if glides { "linear" } else { "hold" }.into(),
                        ),
                    ],
                );
            }
            x.close();
        }
        let sigs: Vec<_> = self.map.signature_changes().collect();
        if !sigs.is_empty() {
            let id = self.id();
            x.open("TimeSignatureAutomation", &[("id", id)]);
            x.empty("Target", &[("parameter", self.signature.clone())]);
            let first = self.map.signature_at(Ticks(0));
            let all = [(Ticks(0), first)]
                .into_iter()
                .chain(sigs.into_iter().map(|(at, _, s)| (at, s)));
            for (at, s) in all {
                x.empty(
                    "TimeSignaturePoint",
                    &[
                        ("time", num(ticks_to_beats(at.0))),
                        ("numerator", s.beats.to_string()),
                        ("denominator", s.unit.to_string()),
                    ],
                );
            }
            x.close();
        }
    }

    /// The audio file behind an asset, read once, and put in the file.
    fn audio_for(&mut self, asset: &str, name: &str) -> Option<AudioInfo> {
        if let Some(a) = self.audio.get(asset) {
            return a.clone();
        }
        let path = (self.o.asset_path)(asset);
        let info = hound::WavReader::open(&path).ok().map(|r| {
            let s = r.spec();
            (s.channels, s.sample_rate, r.duration())
        });
        let out = match info {
            Some((channels, rate, frames)) => {
                let ext = path
                    .extension()
                    .map_or("wav".into(), |e| e.to_string_lossy().into_owned());
                let stem = Path::new(name)
                    .file_stem()
                    .map_or("audio".into(), |s| s.to_string_lossy().into_owned());
                let stem = if stem.is_empty() {
                    "audio".into()
                } else {
                    stem
                };
                let zp = self.free(&format!("audio/{}.{ext}", safe(&stem)));
                let ok = if self.with_files {
                    match std::fs::read(&path) {
                        Ok(d) => {
                            self.files.push((zp.clone(), d));
                            true
                        }
                        Err(_) => false,
                    }
                } else {
                    true
                };
                ok.then(|| AudioInfo {
                    path: zp,
                    channels,
                    rate,
                    seconds: frames as f64 / rate.max(1) as f64,
                })
            }
            None => None,
        };
        if out.is_none() {
            self.report.add(
                "Audio",
                format!("\"{name}\" couldn't be read, so its clips are left out. Saving the project first may help."),
            );
        }
        self.audio.insert(asset.into(), out.clone());
        out
    }

    fn audio_clip(&mut self, x: &mut Xml, _t: &Track, c: &daw_project::Clip) {
        let Some(a) = self.audio_for(&c.asset, &c.name) else {
            return;
        };
        let rate = self.p.rate;
        let start = self.beats(c.start);
        let end = self.beats(c.start + c.len);
        let play_start = frames_to_seconds(c.offset, rate);
        let play_stop = frames_to_seconds(c.offset + c.len, rate);
        x.open(
            "Clip",
            &[
                ("time", num(start)),
                ("duration", num(end - start)),
                ("contentTimeUnit", "seconds".into()),
                ("playStart", num(play_start)),
                ("playStop", num(play_stop)),
                ("fadeTimeUnit", "seconds".into()),
                ("fadeInTime", num(frames_to_seconds(c.fade_in.len, rate))),
                ("fadeOutTime", num(frames_to_seconds(c.fade_out.len, rate))),
                ("enable", if c.muted { b(false) } else { String::new() }),
                ("name", c.name.clone()),
            ],
        );
        let id = self.id();
        x.open(
            "Audio",
            &[
                ("channels", a.channels.to_string()),
                ("sampleRate", a.rate.to_string()),
                ("duration", num(a.seconds)),
                ("timeUnit", "seconds".into()),
                ("id", id),
            ],
        );
        x.empty("File", &[("path", a.path)]);
        x.close();
        x.close();
    }

    fn note_clip(&mut self, x: &mut Xml, c: &NoteClip) {
        x.open(
            "Clip",
            &[
                ("time", num(ticks_to_beats(c.start))),
                ("duration", num(ticks_to_beats(c.len))),
                ("playStart", num(ticks_to_beats(c.offset))),
                ("enable", if c.muted { b(false) } else { String::new() }),
                ("name", c.name.clone()),
            ],
        );
        let controllers: Vec<&Lane> = (c.controllers.iter())
            .filter(|l| !l.points.is_empty() && Controller::from_param(&l.param).is_some())
            .collect();
        let in_lanes = !controllers.is_empty();
        if in_lanes {
            let id = self.id();
            x.open("Lanes", &[("id", id)]);
        }
        let id = self.id();
        x.open("Notes", &[("id", id)]);
        for n in &c.notes {
            self.note(x, n);
        }
        x.close();
        for l in controllers {
            let c = Controller::from_param(&l.param).expect("checked above");
            let (target, scale): (Vec<(&str, String)>, fn(f64) -> f64) = match c {
                Controller::Cc(n) => (
                    vec![
                        ("expression", "channelController".into()),
                        ("channel", "0".into()),
                        ("controller", n.to_string()),
                    ],
                    |v| v / 127.0,
                ),
                Controller::PitchBend => (
                    vec![("expression", "pitchBend".into()), ("channel", "0".into())],
                    |v| (v + 8192.0) / 16383.0,
                ),
                Controller::Pressure => (
                    vec![
                        ("expression", "channelPressure".into()),
                        ("channel", "0".into()),
                    ],
                    |v| v / 127.0,
                ),
            };
            let pts: Vec<(f64, f64)> = (l.points.iter())
                .map(|p| (ticks_to_beats(p.at), scale(p.value).clamp(0.0, 1.0)))
                .collect();
            self.points(x, &target, "normalized", &pts);
        }
        if in_lanes {
            x.close();
        }
        x.close();
    }

    fn note(&mut self, x: &mut Xml, n: &Note) {
        let attrs = [
            ("time", num(ticks_to_beats(n.start))),
            ("duration", num(ticks_to_beats(n.len))),
            ("channel", n.channel.to_string()),
            ("key", n.pitch.to_string()),
            ("vel", num(n.velocity as f64 / 127.0)),
            ("rel", num(n.velocity as f64 / 127.0)),
        ];
        let expr: Vec<&Lane> = (Note::EXPRESSIONS.iter())
            .filter_map(|k| n.expression(k))
            .collect();
        if expr.is_empty() {
            x.empty("Note", &attrs);
            return;
        }
        x.open("Note", &attrs);
        if expr.len() > 1 {
            let id = self.id();
            x.open("Lanes", &[("id", id)]);
        }
        for l in &expr {
            let (kind, unit) = match l.param.as_str() {
                Note::PITCH => ("transpose", "semitones"),
                Note::PRESSURE => ("pressure", "normalized"),
                _ => ("timbre", "normalized"),
            };
            let target = [
                ("expression", kind.to_string()),
                ("channel", n.channel.to_string()),
                ("key", n.pitch.to_string()),
            ];
            let pts: Vec<(f64, f64)> = (l.points.iter())
                .map(|p| (ticks_to_beats(p.at), p.value))
                .collect();
            self.points(x, &target, unit, &pts);
        }
        if expr.len() > 1 {
            x.close();
        }
        x.close();
    }

    /// A line of points, joined by straight lines.
    fn points(&mut self, x: &mut Xml, target: &[(&str, String)], unit: &str, pts: &[(f64, f64)]) {
        let id = self.id();
        x.open("Points", &[("unit", unit.into()), ("id", id)]);
        x.empty("Target", target);
        for (at, v) in pts {
            x.empty(
                "RealPoint",
                &[
                    ("time", num(*at)),
                    ("value", num(*v)),
                    ("interpolation", "linear".into()),
                ],
            );
        }
        x.close();
    }

    fn automation(&mut self, x: &mut Xml, t: &Track, l: &Lane) {
        let ids = &self.ids[&t.id];
        let name = format!("Track \"{}\"", t.name);
        let (param, unit, conv): (Option<String>, &str, Box<dyn Fn(f64) -> f64>) = match l.target()
        {
            LaneTarget::Gain => (
                Some(ids.volume.clone()),
                "linear",
                Box::new(time::db_to_linear),
            ),
            LaneTarget::Pan => (
                Some(ids.pan.clone()),
                "normalized",
                Box::new(|v: f64| (v.clamp(-1.0, 1.0) + 1.0) / 2.0),
            ),
            LaneTarget::Send(bus) => (
                ids.sends.get(bus).cloned(),
                "linear",
                Box::new(time::db_to_linear),
            ),
            LaneTarget::Fx { effect, key } => {
                let p = self.params.get(&(t.id.clone(), l.param.clone())).cloned();
                let e = t.effects.iter().find(|e| e.id == effect);
                let g = e.and_then(|e| Generic::of(&e.builtin));
                let gp = g.and_then(|g| g.params().iter().find(|gp| gp.key == key));
                let in_band = EQ_BANDS.iter().find_map(|b| {
                    if b.freq == key {
                        Some("hertz")
                    } else if b.gain == Some(key) {
                        Some("decibel")
                    } else if b.q == Some(key) {
                        Some("linear")
                    } else {
                        None
                    }
                });
                match (gp, g) {
                    (Some(gp), _) => (p, gp.unit, Box::new(gp.to)),
                    (None, Some(Generic::Eq)) if in_band.is_some() => {
                        (p, in_band.unwrap_or("linear"), Box::new(|v| v))
                    }
                    (None, Some(_)) => (None, "linear", Box::new(|v| v)),
                    (None, None) => (
                        p,
                        e.map_or("linear", |e| devices::unit_of(&e.builtin, key)),
                        Box::new(|v| v),
                    ),
                }
            }
            LaneTarget::Plugin { .. } => (
                self.params.get(&(t.id.clone(), l.param.clone())).cloned(),
                "normalized",
                Box::new(|v: f64| v),
            ),
            LaneTarget::Unknown => (None, "linear", Box::new(|v| v)),
        };
        let Some(param) = param else {
            self.report
                .add(name, format!("Its \"{}\" automation is left out.", l.param));
            return;
        };
        let pts: Vec<(f64, f64)> = (l.points.iter())
            .map(|p| (self.beats(p.at), conv(p.value)))
            .collect();
        self.points(x, &[("parameter", param)], unit, &pts);
    }
}

/// Whether a clip plays: not a take left out of the comp that plays.
fn plays(t: &Track, c: &daw_project::Clip) -> bool {
    if t.comp.is_some() {
        c.comp || c.take.is_none()
    } else {
        !c.comp
    }
}

fn volume_attrs(db: f64, id: String) -> Vec<(&'static str, String)> {
    vec![
        ("max", "2".into()),
        ("min", "0".into()),
        ("unit", "linear".into()),
        ("value", num(time::db_to_linear(db))),
        ("id", id),
        ("name", "Volume".into()),
    ]
}

fn pan_attrs(pan: f64, id: String) -> Vec<(&'static str, String)> {
    vec![
        ("max", "1".into()),
        ("min", "0".into()),
        ("unit", "normalized".into()),
        ("value", num((pan.clamp(-1.0, 1.0) + 1.0) / 2.0)),
        ("id", id),
        ("name", "Pan".into()),
    ]
}

/// A name fit for a file inside the ZIP.
fn safe(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_.()".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let s = s.trim().trim_matches('.').to_string();
    if s.is_empty() { "file".into() } else { s }
}
