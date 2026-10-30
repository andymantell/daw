// SPDX-License-Identifier: GPL-3.0-or-later
//! Reading a DAWproject file into a project.
//!
//! Every place in the file is turned into a frame on our timeline (or
//! a tick, for notes) through the tempo map read from the file, so a
//! clip lands where the other DAW played it. A clip's content is read
//! through the clip: clips inside clips are flattened, cut to the clips
//! around them, and a looped clip is laid out once per pass of its
//! loop. Audio warped to fit the tempo plays at its own speed, starting
//! where it started, and the report says so.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use daw_engine::time::TempoMap;
use daw_project::{
    AutoPoint, Clip, Controller, Curve, Effect, EffectState, Fade, Lane, Marker, MeterChange, Note,
    NoteClip, PPQ, Project, Send, TempoChange, Track, TrackKind, new_id,
};
use roxmltree::{Document, Node};

use crate::devices::{self, EQ_BANDS, Generic, OWN};
use crate::time::{self, beats_to_frames, frames_to_beats, linear_to_db, parse_num};
use crate::{ImportOptions, Imported, Report};

/// Reads a `.dawproject` file's bytes.
pub fn import(bytes: &[u8], o: &ImportOptions) -> Result<Imported, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| format!("this isn't a project file from another DAW ({e})"))?;
    let project = read_entry(&mut zip, "project.xml").ok_or("the file has no project.xml in it")?;
    let project = String::from_utf8(project).map_err(|_| "project.xml isn't text")?;
    let doc = Document::parse(&project).map_err(|e| format!("project.xml is damaged: {e}"))?;
    let title = read_entry(&mut zip, "metadata.xml")
        .and_then(|m| String::from_utf8(m).ok())
        .and_then(|m| {
            let d = Document::parse(&m).ok()?;
            let t = child(d.root_element(), "Title")?.text()?.trim().to_string();
            (!t.is_empty()).then_some(t)
        });
    let root = doc.root_element();
    if root.tag_name().name() != "Project" {
        return Err("project.xml doesn't hold a project".into());
    }
    let mut r = Reader {
        o,
        zip,
        ids: HashMap::new(),
        p: Project {
            rate: o.rate,
            ..Project::default()
        },
        map: TempoMap::new(o.rate.max(1), time::tempo(120.0), time::signature(4, 4)),
        tracks: HashMap::new(),
        channels: HashMap::new(),
        master: Vec::new(),
        targets: HashMap::new(),
        pending_sends: Vec::new(),
        pending_outputs: Vec::new(),
        extracted: HashMap::new(),
        report: Report::default(),
    };
    for n in root.descendants() {
        if let Some(id) = n.attribute("id") {
            r.ids.insert(id.to_string(), n);
        }
    }
    let version = root.attribute("version").unwrap_or("");
    if !version.starts_with("1.") {
        r.report.add(
            "File",
            format!("It is version {version} of the format; this app reads 1.0, so some of it may be missed."),
        );
    }
    let application = child(root, "Application")
        .map(|a| {
            format!(
                "{} {}",
                a.attribute("name").unwrap_or(""),
                a.attribute("version").unwrap_or("")
            )
            .trim()
            .to_string()
        })
        .unwrap_or_default();
    r.transport(root);
    if let Some(s) = child(root, "Structure") {
        for n in s.children().filter(Node::is_element) {
            match n.tag_name().name() {
                "Track" => r.track(n, None),
                "Channel" => r.channel_only(n),
                _ => {}
            }
        }
    }
    r.routing();
    if let Some(a) = child(root, "Arrangement") {
        r.arrangement(a);
    }
    if child(root, "Scenes").is_some_and(|s| s.children().any(|c| c.is_element())) {
        r.report.add(
            "Scenes",
            "Clips in the clip launcher's scenes are left out; the arrangement comes in.",
        );
    }
    let mut p = r.p;
    for t in &mut p.tracks {
        t.note_clips.sort_by_key(|c| c.start);
        for c in &mut t.note_clips {
            c.sort_notes();
        }
        for l in &mut t.automation {
            l.points.sort_by_key(|p| p.at);
        }
    }
    Ok(Imported {
        project: p,
        title,
        application,
        report: r.report,
    })
}

fn read_entry(zip: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<Vec<u8>> {
    let mut f = zip.by_name(name).ok()?;
    let mut out = Vec::new();
    f.read_to_end(&mut out).ok()?;
    Some(out)
}

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.tag_name().name() == name)
}

fn children<'a, 'i>(n: Node<'a, 'i>, name: &'static str) -> impl Iterator<Item = Node<'a, 'i>> {
    n.children().filter(move |c| c.tag_name().name() == name)
}

fn attr_num(n: Node, a: &str) -> Option<f64> {
    n.attribute(a).and_then(parse_num)
}

fn attr_bool(n: Node, a: &str) -> Option<bool> {
    n.attribute(a).map(|v| v == "true" || v == "1")
}

/// A parameter element's value, or `None` when it has none.
fn value(n: Option<Node>) -> Option<f64> {
    attr_num(n?, "value")
}

/// A level parameter (volume, a send) in dB, from its unit.
fn level_db(n: Node) -> Option<f64> {
    let v = attr_num(n, "value")?;
    Some(match n.attribute("unit") {
        Some("decibel") => v.max(-144.0),
        Some("normalized") | Some("percent") => {
            let v = if n.attribute("unit") == Some("percent") {
                v / 100.0
            } else {
                v
            };
            // Over the parameter's range, as a gain.
            let max = attr_num(n, "max").unwrap_or(1.0);
            linear_to_db(v * max)
        }
        _ => linear_to_db(v),
    })
}

/// A pan parameter from −1 to 1, from its range (0 to 1 by default).
fn pan(n: Node) -> Option<f64> {
    let v = attr_num(n, "value")?;
    let (min, max) = match n.attribute("unit") {
        Some("percent") => (
            attr_num(n, "min").unwrap_or(-100.0),
            attr_num(n, "max").unwrap_or(100.0),
        ),
        _ => (
            attr_num(n, "min").unwrap_or(0.0),
            attr_num(n, "max").unwrap_or(1.0),
        ),
    };
    if max <= min {
        return Some(0.0);
    }
    Some(((v - min) / (max - min) * 2.0 - 1.0).clamp(-1.0, 1.0))
}

/// How a point's value becomes ours.
#[derive(Clone, Copy, Debug)]
enum Conv {
    /// A level, by the parameter's unit, to dB.
    Level {
        unit: Unit,
        max: f64,
    },
    /// Pan from its range to −1 to 1.
    Pan {
        min: f64,
        max: f64,
    },
    /// A generic device's setting, through its conversion.
    Generic(fn(f64) -> f64),
    /// From the parameter's range to 0 to 1.
    Normal {
        min: f64,
        max: f64,
    },
    Same,
}

#[derive(Clone, Copy, Debug)]
enum Unit {
    Linear,
    Db,
    Normal,
}

impl Conv {
    fn apply(self, v: f64) -> f64 {
        match self {
            Conv::Level { unit, max } => match unit {
                Unit::Db => v.max(-144.0),
                Unit::Normal => linear_to_db(v * max),
                Unit::Linear => linear_to_db(v),
            },
            Conv::Pan { min, max } if max > min => {
                ((v - min) / (max - min) * 2.0 - 1.0).clamp(-1.0, 1.0)
            }
            Conv::Pan { .. } => 0.0,
            Conv::Generic(f) => f(v),
            Conv::Normal { min, max } if max > min => ((v - min) / (max - min)).clamp(0.0, 1.0),
            Conv::Normal { .. } => v.clamp(0.0, 1.0),
            Conv::Same => v,
        }
    }

    fn level(n: Node) -> Conv {
        Conv::Level {
            unit: match n.attribute("unit") {
                Some("decibel") => Unit::Db,
                Some("normalized") => Unit::Normal,
                _ => Unit::Linear,
            },
            max: attr_num(n, "max").unwrap_or(1.0),
        }
    }

    fn pan(n: Node) -> Conv {
        Conv::Pan {
            min: attr_num(n, "min").unwrap_or(0.0),
            max: attr_num(n, "max").unwrap_or(1.0),
        }
    }
}

/// Where an automated parameter's points go: the track (by its place
/// in the list), the lane and how values convert.
#[derive(Clone, Debug)]
struct Target {
    track: usize,
    param: String,
    conv: Conv,
}

/// Where a timeline's times are counted from, and in what.
#[derive(Clone, Copy, Debug)]
struct Ctx {
    seconds: bool,
    /// The frame and the beat that time 0 of this timeline falls on.
    frame: i64,
    beat: f64,
    /// The window of the timeline that plays, in frames: content
    /// outside the clips around it is cut off.
    lo: i64,
    hi: i64,
}

impl Ctx {
    fn top(seconds: bool) -> Ctx {
        Ctx {
            seconds,
            frame: 0,
            beat: 0.0,
            lo: i64::MIN / 4,
            hi: i64::MAX / 4,
        }
    }
}

struct Reader<'a, 'b, 'i> {
    o: &'b ImportOptions<'b>,
    zip: zip::ZipArchive<Cursor<&'b [u8]>>,
    ids: HashMap<String, Node<'a, 'i>>,
    p: Project,
    map: TempoMap,
    /// Our track's place in the list, by the file's track id.
    tracks: HashMap<String, usize>,
    /// The same, by the file's channel id.
    channels: HashMap<String, usize>,
    /// The master channel's ids.
    master: Vec<String>,
    /// Automated parameters, by the file's parameter id.
    targets: HashMap<String, Target>,
    /// Sends and outputs, joined up once every track is read: track,
    /// destination channel, and the send.
    pending_sends: Vec<(usize, String, Send)>,
    pending_outputs: Vec<(usize, String)>,
    /// Files taken out of the ZIP, by their path in it.
    extracted: HashMap<String, Option<String>>,
    report: Report,
}

impl<'a, 'b, 'i> Reader<'a, 'b, 'i> {
    fn ctx_frame(&self, c: &Ctx, t: f64) -> i64 {
        if c.seconds {
            c.frame + time::seconds_to_frames(t, self.p.rate)
        } else {
            beats_to_frames(&self.map, c.beat + t)
        }
    }

    fn ctx_tick(&self, c: &Ctx, t: f64) -> i64 {
        if c.seconds {
            time::frames_to_ticks(&self.map, self.ctx_frame(c, t))
        } else {
            time::beats_to_ticks(c.beat + t)
        }
    }

    /// The context inside a timeline whose time 0 is at `t` here, in
    /// `seconds` or beats.
    fn inner(&self, c: &Ctx, t: f64, seconds: bool) -> Ctx {
        let frame = self.ctx_frame(c, t);
        let beat = if c.seconds {
            frames_to_beats(&self.map, frame)
        } else {
            c.beat + t
        };
        Ctx {
            seconds,
            frame,
            beat,
            ..*c
        }
    }

    fn transport(&mut self, root: Node) {
        let arr = child(root, "Arrangement");
        let tr = child(root, "Transport");
        let bpm = tr.and_then(|t| value(child(t, "Tempo"))).unwrap_or(120.0);
        let sig = tr.and_then(|t| child(t, "TimeSignature"));
        let (mut beats, mut unit) = sig.map_or((4, 4), |s| {
            (
                attr_num(s, "numerator").unwrap_or(4.0) as u32,
                attr_num(s, "denominator").unwrap_or(4.0) as u32,
            )
        });
        let seconds = arr
            .and_then(|a| child(a, "Lanes"))
            .is_some_and(|l| l.attribute("timeUnit") == Some("seconds"));
        // Time signatures first: they decide where the bars are.
        let mut meters: Vec<(f64, u32, u32)> = Vec::new();
        if let Some(sa) = arr.and_then(|a| child(a, "TimeSignatureAutomation")) {
            for p in children(sa, "TimeSignaturePoint") {
                let (Some(at), Some(n), Some(d)) = (
                    attr_num(p, "time"),
                    attr_num(p, "numerator"),
                    attr_num(p, "denominator"),
                ) else {
                    continue;
                };
                meters.push((at, n as u32, d as u32));
            }
        }
        meters.sort_by(|a, b| a.0.total_cmp(&b.0));
        if let Some(first) = meters.first()
            && first.0 <= 0.0
        {
            (beats, unit) = (first.1, first.2);
        }
        let mut tempos: Vec<(f64, f64, bool)> = Vec::new();
        if let Some(ta) = arr.and_then(|a| child(a, "TempoAutomation")) {
            for p in children(ta, "RealPoint") {
                let (Some(at), Some(v)) = (attr_num(p, "time"), attr_num(p, "value")) else {
                    continue;
                };
                let linear = p.attribute("interpolation") == Some("linear");
                tempos.push((at, v, linear));
            }
        }
        tempos.sort_by(|a, b| a.0.total_cmp(&b.0));
        let start = match tempos.first() {
            Some(f) if f.0 <= 0.0 => f.1,
            _ => bpm,
        };
        self.p.tempo_bpm = (start * 1000.0).round() / 1000.0;
        self.p.beats_per_bar = beats.clamp(1, 64);
        self.p.beat_unit = time::signature(beats, unit).unit;
        if seconds && (tempos.len() > 1 || meters.len() > 1) {
            self.report.add(
                "Tempo",
                "The file counts its changes of tempo in seconds; they come in rounded to the nearest beat.",
            );
        }
        // Each change is placed by bar and beat on the map made so far.
        let mut map = time::tempo_map(&self.p);
        let to_beats = |m: &TempoMap, t: f64| {
            if seconds {
                frames_to_beats(m, time::seconds_to_frames(t, self.p.rate))
            } else {
                t
            }
        };
        for (at, n, d) in meters.iter().copied().filter(|m| m.0 > 0.0) {
            let tick = time::beats_to_ticks(to_beats(&map, at));
            let bb = map.bar_beat(daw_engine::time::Ticks(tick));
            let bar = if bb.beat == 1 && bb.tick == 0 {
                bb.bar
            } else {
                self.report.add(
                    "Time signature",
                    "A change of time signature away from a bar line moves to the next bar.",
                );
                bb.bar + 1
            };
            self.p.meter_changes.push(MeterChange {
                id: new_id(),
                bar,
                beats: n.clamp(1, 64),
                unit: time::signature(n, d).unit,
            });
            map = time::tempo_map(&self.p);
        }
        let mut prev_linear = tempos.first().is_some_and(|f| f.0 <= 0.0 && f.2);
        for (i, (at, v, linear)) in tempos.iter().copied().enumerate() {
            if i == 0 && at <= 0.0 {
                continue;
            }
            let tick = time::beats_to_ticks(to_beats(&map, at));
            let bb = map.bar_beat(daw_engine::time::Ticks(tick));
            if bb.tick != 0 {
                self.report.add(
                    "Tempo",
                    "A change of tempo between beats moves to the beat before it.",
                );
            }
            // The line before this point glides into it.
            let glide = prev_linear;
            self.p.tempo_changes.push(TempoChange {
                id: new_id(),
                bar: bb.bar,
                beat: bb.beat as u32,
                bpm: (v.clamp(10.0, 999.0) * 1000.0).round() / 1000.0,
                glide,
            });
            prev_linear = linear;
        }
        self.map = time::tempo_map(&self.p);
    }

    /// A track and the tracks inside it.
    fn track(&mut self, n: Node<'a, 'i>, parent: Option<usize>) {
        let name = n.attribute("name").unwrap_or("Track").to_string();
        let content = n.attribute("contentType").unwrap_or("");
        let inner: Vec<Node> = children(n, "Track").collect();
        let ch = child(n, "Channel");
        let role = ch.and_then(|c| c.attribute("role")).unwrap_or("regular");
        if role == "master" {
            if let Some(id) = n.attribute("id") {
                self.master.push(id.into());
            }
            if let Some(c) = ch {
                if let Some(id) = c.attribute("id") {
                    self.master.push(id.into());
                }
                if child(c, "Devices").is_some_and(|d| d.children().any(|e| e.is_element())) {
                    self.report.add(
                        "Master",
                        "Effects on the master are left out; this app has none there yet.",
                    );
                }
            }
            return;
        }
        let kind = if !inner.is_empty() || content.split(' ').any(|c| c == "tracks") {
            TrackKind::Folder
        } else if matches!(role, "effect" | "submix") {
            TrackKind::Bus
        } else if content.split(' ').any(|c| c == "notes") {
            TrackKind::Instrument
        } else {
            TrackKind::Audio
        };
        if role == "vca" {
            self.report.add(
                format!("Track \"{name}\""),
                "It controls other tracks' levels in the other DAW; it comes in as a track of its own.",
            );
        }
        let mut t = Track::new(new_id(), name);
        t.kind = kind;
        t.parent = parent.map(|i| self.p.tracks[i].id.clone());
        let i = self.p.tracks.len();
        self.p.tracks.push(t);
        if let Some(id) = n.attribute("id") {
            self.tracks.insert(id.into(), i);
        }
        if let Some(c) = ch {
            self.channel(c, i);
        }
        for c in inner {
            self.track(c, Some(i));
        }
    }

    /// A channel with no track: a bus.
    fn channel_only(&mut self, c: Node<'a, 'i>) {
        if c.attribute("role") == Some("master") {
            if let Some(id) = c.attribute("id") {
                self.master.push(id.into());
            }
            return;
        }
        let name = c.attribute("name").unwrap_or("Bus").to_string();
        let mut t = Track::new(new_id(), name);
        t.kind = TrackKind::Bus;
        let i = self.p.tracks.len();
        self.p.tracks.push(t);
        self.channel(c, i);
    }

    fn channel(&mut self, c: Node<'a, 'i>, i: usize) {
        if let Some(id) = c.attribute("id") {
            self.channels.insert(id.into(), i);
        }
        let t = &mut self.p.tracks[i];
        t.solo = attr_bool(c, "solo").unwrap_or(false);
        if let Some(m) = child(c, "Mute") {
            t.mute = attr_bool(m, "value").unwrap_or(false);
        }
        if let Some(v) = child(c, "Volume") {
            t.gain_db = level_db(v).unwrap_or(0.0).clamp(-144.0, 24.0);
            if let Some(id) = v.attribute("id") {
                self.targets.insert(
                    id.into(),
                    Target {
                        track: i,
                        param: Lane::GAIN.into(),
                        conv: Conv::level(v),
                    },
                );
            }
        }
        if let Some(p) = child(c, "Pan") {
            t.pan = pan(p).unwrap_or(0.0);
            if let Some(id) = p.attribute("id") {
                self.targets.insert(
                    id.into(),
                    Target {
                        track: i,
                        param: Lane::PAN.into(),
                        conv: Conv::pan(p),
                    },
                );
            }
        }
        if let Some(m) = child(c, "Mute")
            && let Some(id) = m.attribute("id")
        {
            // Points on it are told about when they are found.
            self.targets.insert(
                id.into(),
                Target {
                    track: i,
                    param: "mute".into(),
                    conv: Conv::Same,
                },
            );
        }
        if let Some(d) = c.attribute("destination") {
            self.pending_outputs.push((i, d.into()));
        }
        if let Some(sends) = child(c, "Sends") {
            for s in children(sends, "Send") {
                let Some(dest) = s.attribute("destination") else {
                    continue;
                };
                let mut send = Send::to(String::new());
                send.pre_fader = s.attribute("type") == Some("pre");
                if let Some(v) = child(s, "Volume") {
                    send.gain_db = level_db(v).unwrap_or(0.0).clamp(-144.0, 24.0);
                    if let Some(id) = v.attribute("id") {
                        // The bus is filled in once known.
                        self.targets.insert(
                            id.into(),
                            Target {
                                track: i,
                                param: format!("{}?{dest}", Lane::SEND),
                                conv: Conv::level(v),
                            },
                        );
                    }
                }
                if let Some(e) = child(s, "Enable") {
                    send.mute = attr_bool(e, "value") == Some(false);
                }
                if child(s, "Pan").is_some() {
                    self.report.add(
                        format!("Track \"{}\"", self.p.tracks[i].name),
                        "A send's own pan is left out; sends take the track's pan.",
                    );
                }
                self.pending_sends.push((i, dest.into(), send));
            }
        }
        if let Some(d) = child(c, "Devices") {
            for e in d.children().filter(Node::is_element) {
                self.device(e, i);
            }
        }
    }

    /// Joins up outputs and sends once every channel is known.
    fn routing(&mut self) {
        for (i, dest) in std::mem::take(&mut self.pending_outputs) {
            if self.master.contains(&dest) {
                continue;
            }
            let Some(&to) = self.channels.get(&dest).or(self.tracks.get(&dest)) else {
                continue;
            };
            let parent = self.p.tracks[i].parent.clone();
            let to_id = self.p.tracks[to].id.clone();
            if parent.as_deref() == Some(to_id.as_str()) || to == i {
                continue;
            }
            if !self.p.tracks[to].kind.is_bus() {
                // Something routed into a plain track: it becomes a bus,
                // keeping any clips it has.
                self.p.tracks[to].kind = TrackKind::Bus;
            }
            self.p.tracks[i].output = Some(to_id);
        }
        let mut send_ids: HashMap<String, String> = HashMap::new();
        for (i, dest, mut s) in std::mem::take(&mut self.pending_sends) {
            let Some(&to) = self.channels.get(&dest).or(self.tracks.get(&dest)) else {
                self.report.add(
                    format!("Track \"{}\"", self.p.tracks[i].name),
                    "A send to the master or to nowhere is left out.",
                );
                continue;
            };
            if !self.p.tracks[to].kind.is_bus() {
                self.p.tracks[to].kind = TrackKind::Bus;
            }
            s.to = self.p.tracks[to].id.clone();
            send_ids.insert(dest.clone(), s.to.clone());
            self.p.tracks[i].sends.push(s);
        }
        for t in self.targets.values_mut() {
            if let Some(dest) = t.param.strip_prefix(&format!("{}?", Lane::SEND)) {
                t.param = match send_ids.get(dest) {
                    Some(bus) => format!("{}{bus}", Lane::SEND),
                    None => String::new(),
                };
            }
        }
    }

    fn device(&mut self, d: Node<'a, 'i>, i: usize) {
        let tag = d.tag_name().name();
        let role = d.attribute("deviceRole").unwrap_or("audioFX");
        let name = (d.attribute("name"))
            .or(d.attribute("deviceName"))
            .unwrap_or(tag)
            .to_string();
        let track_name = format!("Track \"{}\"", self.p.tracks[i].name);
        let device_id = d.attribute("deviceID").unwrap_or("");
        let enabled = child(d, "Enabled").and_then(|e| attr_bool(e, "value"));
        let state_path = child(d, "State").and_then(|s| {
            let ext = attr_bool(s, "external").unwrap_or(false);
            Some((s.attribute("path")?.to_string(), ext))
        });
        let mut e = Effect {
            id: new_id(),
            name: name.clone(),
            bypass: enabled == Some(false),
            ..Effect::default()
        };
        // Parameters listed for automation: plugin parameters by their
        // number, settings of our own devices by name.
        let mut params: Vec<(String, String, Conv)> = Vec::new();
        match tag {
            "Vst3Plugin" | "ClapPlugin" => {
                let clap = tag == "ClapPlugin";
                e.class = if clap {
                    devices::clap_class(device_id)
                } else {
                    match devices::class_from_hex(device_id) {
                        Some(c) => c,
                        None => {
                            self.report.add(
                                &track_name,
                                format!(
                                    "The plugin \"{name}\" has no usable id, so it is left out."
                                ),
                            );
                            return;
                        }
                    }
                };
                if clap {
                    // Found again by its class id; the extension says
                    // which format to load it as.
                    e.bundle = format!("{name}.clap");
                }
                match state_path.and_then(|(p, ext)| self.file_bytes(&p, ext)) {
                    Some(bytes) if clap => {
                        e.state = Some(EffectState {
                            component: bytes,
                            controller: None,
                        })
                    }
                    Some(bytes) => match daw_plugins::presets::PresetFile::decode(&bytes) {
                        Ok(f) => {
                            e.state = Some(EffectState {
                                component: f.state.component,
                                controller: f.state.controller,
                            })
                        }
                        Err(err) => self.report.add(
                            &track_name,
                            format!("The plugin \"{name}\" comes in with its own defaults: its settings couldn't be read ({err})."),
                        ),
                    },
                    None => self.report.add(
                        &track_name,
                        format!("The plugin \"{name}\" comes in with its own defaults: the file has no settings for it."),
                    ),
                }
                if !(self.o.has_plugin)(&e) {
                    self.report.add(
                        &track_name,
                        format!("The plugin \"{name}\" isn't on this computer. It keeps its settings, and plays once it is installed."),
                    );
                }
                if let Some(ps) = child(d, "Parameters") {
                    for p in ps.children().filter(Node::is_element) {
                        let (Some(id), Some(num)) = (p.attribute("id"), p.attribute("parameterID"))
                        else {
                            continue;
                        };
                        let conv = Conv::Normal {
                            min: attr_num(p, "min").unwrap_or(0.0),
                            max: attr_num(p, "max").unwrap_or(1.0),
                        };
                        params.push((id.into(), format!("{}{}:{num}", Lane::PLUGIN, e.id), conv));
                    }
                }
            }
            "Vst2Plugin" | "AuPlugin" => {
                let format = if tag == "AuPlugin" { "AU" } else { "VST 2" };
                self.report.add(
                    &track_name,
                    format!("The {format} plugin \"{name}\" is left out: this app hosts VST 3 and CLAP plugins."),
                );
                return;
            }
            _ => {
                let own = device_id.strip_prefix(OWN);
                let restored = match (own, state_path) {
                    (Some(_), Some((p, ext))) => self
                        .file_bytes(&p, ext)
                        .and_then(|b| devices::from_own_state(&b)),
                    _ => None,
                };
                if let Some(mut r) = restored {
                    // The sound files it uses come out of the ZIP.
                    let mut files: Vec<String> = Vec::new();
                    devices::map_files(&mut r, &mut |f| {
                        files.push(f.into());
                        f.into()
                    });
                    let mut found: HashMap<String, String> = HashMap::new();
                    for f in files {
                        if let Some(z) = f.strip_prefix("zip:")
                            && let Some(path) = self.extract(z)
                        {
                            found.insert(f.clone(), path);
                        }
                    }
                    devices::map_files(&mut r, &mut |f| {
                        found.get(f).cloned().unwrap_or_else(|| f.into())
                    });
                    r.id = e.id.clone();
                    e = Effect {
                        bypass: e.bypass,
                        ..r
                    };
                } else if let Some(g) = Generic::from_element(tag) {
                    e.builtin = g.builtin().into();
                    self.generic(d, g, &mut e, &track_name, &mut params);
                } else {
                    self.report.add(
                        &track_name,
                        format!(
                            "\"{name}\" is a device of the other DAW's own, so it is left out."
                        ),
                    );
                    return;
                }
                if let Some(ps) = child(d, "Parameters") {
                    for p in ps.children().filter(Node::is_element) {
                        if let (Some(id), Some(key)) = (p.attribute("id"), p.attribute("name")) {
                            params.push((
                                id.into(),
                                format!("{}{}:{key}", Lane::FX, e.id),
                                Conv::Same,
                            ));
                        }
                    }
                }
                if !e.builtin.is_empty()
                    && let Some(g) = Generic::of(&e.builtin)
                {
                    // A generic device of ours: its elements name
                    // settings too. Their values are in the state.
                    let mut copy = e.clone();
                    self.generic(d, g, &mut copy, "", &mut params);
                }
            }
        }
        for (id, param, conv) in params {
            self.targets.insert(
                id,
                Target {
                    track: i,
                    param,
                    conv,
                },
            );
        }
        let t = &mut self.p.tracks[i];
        match role {
            "instrument" => {
                if t.instrument.is_some() {
                    self.report.add(
                        &track_name,
                        format!("\"{name}\" is a second instrument on the track, so it is left out."),
                    );
                    return;
                }
                if t.kind == TrackKind::Audio {
                    t.kind = TrackKind::Instrument;
                }
                t.instrument = Some(e);
            }
            "noteFX" => self.report.add(
                &track_name,
                format!("\"{name}\" works on notes before the instrument; this app has no place for it yet, so it is left out."),
            ),
            _ => t.effects.push(e),
        }
    }

    /// One of the four generic devices, setting by setting, as ours.
    fn generic(
        &mut self,
        d: Node,
        g: Generic,
        e: &mut Effect,
        track_name: &str,
        params: &mut Vec<(String, String, Conv)>,
    ) {
        let lane = |e: &Effect, key: &str| format!("{}{}:{key}", Lane::FX, e.id);
        let clamp = |r: &mut Report, e: &Effect, key: &str, v: f64| -> f64 {
            match devices::range(g.builtin(), key) {
                Some((lo, hi)) if v < lo || v > hi => {
                    if !track_name.is_empty() {
                        r.add(
                            track_name,
                            format!("\"{}\": its {key} is beyond what ours goes to, so it is set as near as it goes.", e.name),
                        );
                    }
                    v.clamp(lo, hi)
                }
                _ => v,
            }
        };
        for gp in g.params() {
            let Some(n) = child(d, gp.element) else {
                continue;
            };
            if let Some(v) = value(Some(n)) {
                let v = clamp(&mut self.report, e, gp.key, (gp.from)(v));
                devices::set(e, gp.key, v);
            }
            if let Some(id) = n.attribute("id") {
                params.push((id.into(), lane(e, gp.key), Conv::Generic(gp.from)));
            }
        }
        if track_name.is_empty() {
            if g == Generic::Eq {
                self.eq_bands(d, e, track_name, params);
            }
            return;
        }
        match g {
            Generic::Compressor => {
                if child(d, "InputGain")
                    .and_then(|n| value(Some(n)))
                    .is_some_and(|v| v != 0.0)
                {
                    self.report
                        .add(track_name, "A compressor's input gain is left out.");
                }
                if child(d, "AutoMakeup").and_then(|n| attr_bool(n, "value")) == Some(true) {
                    self.report.add(
                        track_name,
                        "A compressor's automatic make-up gain is left out.",
                    );
                }
            }
            Generic::Limiter => {
                if child(d, "OutputGain")
                    .and_then(|n| value(Some(n)))
                    .is_some_and(|v| v != 0.0)
                {
                    self.report
                        .add(track_name, "A limiter's output gain is left out.");
                }
            }
            Generic::Gate => {
                if child(d, "Ratio").is_some() {
                    self.report.add(
                        track_name,
                        "A gate's ratio is left out: ours closes by its depth.",
                    );
                }
            }
            Generic::Eq => self.eq_bands(d, e, track_name, params),
        }
    }

    /// The bands of a generic EQ, onto our six.
    fn eq_bands(
        &mut self,
        d: Node,
        e: &mut Effect,
        track_name: &str,
        params: &mut Vec<(String, String, Conv)>,
    ) {
        let mut bands: Vec<Node> = children(d, "Band").collect();
        bands.sort_by_key(|b| attr_num(*b, "order").map_or(0, |v| v as i64));
        let mut used = [false; 6];
        for b in bands {
            let kind = b.attribute("type").unwrap_or("");
            let slot = (0..6).find(|&i| !used[i] && EQ_BANDS[i].kind == kind);
            let Some(slot) = slot else {
                if !track_name.is_empty() {
                    self.report.add(
                        track_name,
                        format!("An EQ band ({kind}) has no place in ours, so it is left out."),
                    );
                }
                continue;
            };
            used[slot] = true;
            let band = &EQ_BANDS[slot];
            let on = child(b, "Enabled").and_then(|n| attr_bool(n, "value")) != Some(false);
            let put = |e: &mut Effect, key: &str, v: f64, r: &mut Report| {
                let v = match devices::range("eq", key) {
                    Some((lo, hi)) if v < lo || v > hi => {
                        if !track_name.is_empty() {
                            r.add(
                                track_name,
                                format!("An EQ band's {key} is beyond what ours goes to, so it is set as near as it goes."),
                            );
                        }
                        v.clamp(lo, hi)
                    }
                    _ => v,
                };
                devices::set(e, key, v);
            };
            if let Some(f) = child(b, "Freq") {
                if let Some(v) = value(Some(f)) {
                    let v = match band.off_at {
                        Some(off) if !on => off,
                        _ => v,
                    };
                    put(e, band.freq, v, &mut self.report);
                }
                if let Some(id) = f.attribute("id") {
                    params.push((
                        id.into(),
                        format!("{}{}:{}", Lane::FX, e.id, band.freq),
                        Conv::Same,
                    ));
                }
            }
            if let (Some(k), Some(n)) = (band.gain, child(b, "Gain")) {
                if let Some(v) = value(Some(n)) {
                    put(e, k, if on { v } else { 0.0 }, &mut self.report);
                }
                if let Some(id) = n.attribute("id") {
                    params.push((id.into(), format!("{}{}:{k}", Lane::FX, e.id), Conv::Same));
                }
            }
            if let (Some(k), Some(n)) = (band.q, child(b, "Q")) {
                if let Some(v) = value(Some(n)) {
                    put(e, k, v, &mut self.report);
                }
                if let Some(id) = n.attribute("id") {
                    params.push((id.into(), format!("{}{}:{k}", Lane::FX, e.id), Conv::Same));
                }
            }
        }
    }

    /// A file's bytes: inside the ZIP, or beside it when external.
    fn file_bytes(&mut self, path: &str, external: bool) -> Option<Vec<u8>> {
        if external {
            return std::fs::read(self.outside(path)?).ok();
        }
        let mut f = self.zip.by_name(path).ok()?;
        let mut out = Vec::new();
        f.read_to_end(&mut out).ok()?;
        Some(out)
    }

    /// An external file's full path, if it is there.
    fn outside(&self, path: &str) -> Option<PathBuf> {
        let p = Path::new(path);
        let full = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.o.base_dir.as_ref()?.join(p)
        };
        full.is_file().then_some(full)
    }

    /// Takes a file out of the ZIP into the audio folder, once, and
    /// returns its full path.
    fn extract(&mut self, path: &str) -> Option<String> {
        if let Some(p) = self.extracted.get(path) {
            return p.clone();
        }
        let out = (|| {
            let mut f = self.zip.by_name(path).ok()?;
            let name = Path::new(path).file_name()?.to_string_lossy().into_owned();
            std::fs::create_dir_all(&self.o.audio_dir).ok()?;
            let mut dest = self.o.audio_dir.join(&name);
            let mut n = 2;
            while dest.exists() {
                let p = Path::new(&name);
                let stem = p.file_stem().unwrap_or_default().to_string_lossy();
                let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy()));
                dest = self
                    .o
                    .audio_dir
                    .join(format!("{stem} {n}{}", ext.unwrap_or_default()));
                n += 1;
            }
            let mut file = std::fs::File::create(&dest).ok()?;
            std::io::copy(&mut f, &mut file).ok()?;
            Some(dest.to_string_lossy().into_owned())
        })();
        self.extracted.insert(path.into(), out.clone());
        out
    }

    /// The audio file a `File` element names, as a full path.
    fn audio_file(&mut self, f: Node) -> Option<String> {
        let path = f.attribute("path")?;
        if attr_bool(f, "external").unwrap_or(false) {
            return self.outside(path).map(|p| p.to_string_lossy().into_owned());
        }
        self.extract(path)
    }

    fn arrangement(&mut self, a: Node<'a, 'i>) {
        if let Some(lanes) = child(a, "Lanes") {
            let seconds = lanes.attribute("timeUnit") == Some("seconds");
            self.timeline(lanes, None, &Ctx::top(seconds));
        }
        if let Some(m) = child(a, "Markers") {
            let seconds = m.attribute("timeUnit") == Some("seconds")
                || child(a, "Lanes").is_some_and(|l| l.attribute("timeUnit") == Some("seconds"));
            self.markers(m, &Ctx::top(seconds));
        }
    }

    fn markers(&mut self, m: Node, c: &Ctx) {
        for n in children(m, "Marker") {
            let Some(at) = attr_num(n, "time") else {
                continue;
            };
            let tick = self.ctx_tick(c, at);
            let bb = self.map.bar_beat(daw_engine::time::Ticks(tick));
            if bb.beat != 1 || bb.tick != 0 {
                self.report.add(
                    "Markers",
                    "Markers here sit on bar lines: one between bars moves to the start of its bar.",
                );
            }
            self.p.markers.push(Marker {
                id: new_id(),
                bar: bb.bar.max(1),
                name: n.attribute("name").unwrap_or("").to_string(),
            });
        }
        self.p.markers.sort_by_key(|m| m.bar);
    }

    /// The time unit a timeline sets for itself and what's inside it.
    fn unit(n: Node, c: &Ctx) -> Ctx {
        match n.attribute("timeUnit") {
            Some("seconds") => Ctx {
                seconds: true,
                ..*c
            },
            Some("beats") => Ctx {
                seconds: false,
                ..*c
            },
            _ => *c,
        }
    }

    /// Any timeline, for the track `track` (by place), in context `c`.
    fn timeline(&mut self, n: Node<'a, 'i>, track: Option<usize>, c: &Ctx) {
        let track = n
            .attribute("track")
            .and_then(|t| self.tracks.get(t).or(self.channels.get(t)).copied())
            .or(track);
        let c = Reader::unit(n, c);
        match n.tag_name().name() {
            "Lanes" => {
                for k in n.children().filter(Node::is_element) {
                    self.timeline(k, track, &c);
                }
            }
            "Clips" => {
                for clip in children(n, "Clip") {
                    self.clip(clip, track, &c);
                }
            }
            "Points" => self.track_points(n, track, &c),
            "Notes" | "Audio" | "Warps" => {
                // Content straight on a track: as one clip of all of it.
                if let Some(t) = track {
                    self.content(n, t, &c, None);
                }
            }
            "Markers" => self.markers(n, &c),
            "ClipSlot" => self.report.add(
                "Clip launcher",
                "Clip slots are left out; this app has no clip launcher yet.",
            ),
            "Video" => self.report.add("Video", "Video is left out of the import."),
            _ => {}
        }
    }

    /// A clip on a timeline: its content, cut to its window, once per
    /// pass of its loop.
    fn clip(&mut self, clip: Node<'a, 'i>, track: Option<usize>, c: &Ctx) {
        let Some(t) = track else {
            return;
        };
        let Some(time) = attr_num(clip, "time") else {
            return;
        };
        let content = match clip.attribute("reference") {
            Some(r) => self.ids.get(r).copied(),
            None => clip.children().find(Node::is_element),
        };
        let Some(content) = content else {
            return;
        };
        let inner_seconds = match clip.attribute("contentTimeUnit") {
            Some("seconds") => true,
            Some("beats") => false,
            _ => c.seconds,
        };
        let play_start = attr_num(clip, "playStart").unwrap_or(0.0);
        let play_stop = attr_num(clip, "playStop");
        let start = self.ctx_frame(c, time);
        let end = match attr_num(clip, "duration") {
            Some(d) => self.ctx_frame(c, time + d),
            None => match play_stop {
                Some(stop) => {
                    let inner = self.inner(c, time, inner_seconds);
                    self.ctx_frame(&inner, stop - play_start)
                }
                None => return,
            },
        };
        let (start, end) = (start.max(c.lo), end.min(c.hi));
        if end <= start {
            return;
        }
        let muted = attr_bool(clip, "enable") == Some(false);
        let fade_seconds = match clip.attribute("fadeTimeUnit") {
            Some("seconds") => true,
            Some("beats") => false,
            _ => inner_seconds,
        };
        let fade = |r: &Reader, at: i64, len: Option<f64>| -> i64 {
            let Some(len) = len else { return 0 };
            let len = len.abs();
            if fade_seconds {
                time::seconds_to_frames(len, r.p.rate)
            } else {
                let b = frames_to_beats(&r.map, at);
                (beats_to_frames(&r.map, b + len) - at).abs()
            }
        };
        let fade_in = fade(self, start, attr_num(clip, "fadeInTime"));
        let fade_out = fade(self, end, attr_num(clip, "fadeOutTime"));
        let looping = match (attr_num(clip, "loopStart"), attr_num(clip, "loopEnd")) {
            (Some(a), Some(b)) if b > a => Some((a, b)),
            _ => None,
        };
        // Each pass: where it starts in the content, where it stops, and
        // how far into the clip it plays.
        let base = self.inner(c, time, inner_seconds);
        let length = if inner_seconds {
            time::frames_to_seconds(end - start, self.p.rate)
        } else {
            frames_to_beats(&self.map, end) - base.beat
        };
        let mut passes: Vec<(f64, f64, f64)> = Vec::new();
        let (mut at, mut pos) = (play_start, 0.0);
        let mut guard = 0;
        while pos < length - 1e-9 && guard < 10_000 {
            guard += 1;
            let stop = match looping {
                Some((_, le)) if at < le => le,
                _ => at + (length - pos),
            };
            let stop = stop.min(at + (length - pos));
            if stop <= at {
                break;
            }
            passes.push((at, stop, pos));
            pos += stop - at;
            at = match looping {
                Some((ls, _)) => ls,
                None => stop,
            };
        }
        if passes.len() > 1 {
            self.report.add(
                format!("Track \"{}\"", self.p.tracks[t].name),
                "A looped clip comes in as one clip per pass of its loop.",
            );
        }
        let first_audio = self.p.tracks[t].clips.len();
        let first_notes = self.p.tracks[t].note_clips.len();
        for (a, b, pos) in &passes {
            // The pass's content time `a` falls at `pos` into the clip.
            let ctx = Ctx {
                lo: (start).max(self.ctx_frame(&base, pos - 0.0).min(start)),
                hi: end,
                ..self.inner(&base, *pos - *a, inner_seconds)
            };
            let window = (self.ctx_frame(&ctx, *a), self.ctx_frame(&ctx, *b));
            let ctx = Ctx {
                lo: window.0.max(start),
                hi: window.1.min(end),
                ..ctx
            };
            self.content(content, t, &ctx, Some((*a, *b)));
        }
        // Fades go on the first and last audio of the clip; muting on all.
        let tr = &mut self.p.tracks[t];
        let new_audio = first_audio..tr.clips.len();
        if !new_audio.is_empty() {
            let (first, last) = (new_audio.start, new_audio.end - 1);
            if fade_in > 0 {
                tr.clips[first].fade_in = Fade {
                    len: fade_in.min(tr.clips[first].len),
                    curve: Curve::Linear,
                };
            }
            if fade_out > 0 {
                tr.clips[last].fade_out = Fade {
                    len: fade_out.min(tr.clips[last].len),
                    curve: Curve::Linear,
                };
            }
            let name = clip.attribute("name").unwrap_or("");
            for cl in &mut tr.clips[new_audio] {
                cl.muted |= muted;
                if !name.is_empty() {
                    cl.name = name.into();
                }
            }
        }
        let name = clip.attribute("name").unwrap_or("");
        for nc in &mut tr.note_clips[first_notes..] {
            nc.muted |= muted;
            if !name.is_empty() {
                nc.name = name.into();
            }
        }
    }

    /// A clip's content, within `window` of its own time (all of it for
    /// `None`), placed through `c`.
    fn content(&mut self, n: Node<'a, 'i>, t: usize, c: &Ctx, window: Option<(f64, f64)>) {
        let c = Reader::unit(n, c);
        match n.tag_name().name() {
            "Notes" => {
                let mut nc = self.note_clip(t, &c, window);
                self.notes(n, &c, window, &mut nc);
                self.push_note_clip(t, nc);
            }
            "Points" => {
                // Controller lines in a clip, or automation.
                let expr = child(n, "Target").and_then(|tg| tg.attribute("expression"));
                if expr.is_some() {
                    let mut nc = self.note_clip(t, &c, window);
                    self.controller(n, &c, window, &mut nc);
                    self.push_note_clip(t, nc);
                } else {
                    self.track_points(n, Some(t), &c);
                }
            }
            "Lanes" => {
                // Notes and controller lines together make one note clip.
                let has_notes = n.children().any(|k| k.tag_name().name() == "Notes");
                let mut nc = has_notes.then(|| self.note_clip(t, &c, window));
                for k in n.children().filter(Node::is_element) {
                    let kc = Reader::unit(k, &c);
                    match (k.tag_name().name(), nc.as_mut()) {
                        ("Notes", Some(nc)) => self.notes(k, &kc, window, nc),
                        ("Points", Some(nc))
                            if child(k, "Target")
                                .and_then(|tg| tg.attribute("expression"))
                                .is_some() =>
                        {
                            self.controller(k, &kc, window, nc)
                        }
                        _ => self.content(k, t, &kc, window),
                    }
                }
                if let Some(nc) = nc {
                    self.push_note_clip(t, nc);
                }
            }
            "Clips" => {
                for clip in children(n, "Clip") {
                    self.clip(clip, Some(t), &c);
                }
            }
            "Audio" => self.audio(n, t, &c, window, None),
            "Warps" => self.warps(n, t, &c, window),
            "ClipSlot" => self.report.add(
                "Clip launcher",
                "Clip slots are left out; this app has no clip launcher yet.",
            ),
            "Video" => self.report.add("Video", "Video is left out of the import."),
            _ => {}
        }
    }

    fn note_clip(&self, _t: usize, c: &Ctx, window: Option<(f64, f64)>) -> NoteClip {
        let lo = (c.lo).max(i64::MIN / 8);
        let start_tick = match window {
            Some((a, _)) => self.ctx_tick(c, a),
            None if lo > i64::MIN / 8 => time::frames_to_ticks(&self.map, lo),
            None => 0,
        };
        let start_tick = start_tick.max(time::frames_to_ticks(&self.map, c.lo.max(0)));
        let end_tick = match window {
            Some((_, b)) => self.ctx_tick(c, b),
            None => start_tick,
        };
        let end_tick = if c.hi < i64::MAX / 8 {
            end_tick.min(time::frames_to_ticks(&self.map, c.hi))
        } else {
            end_tick
        };
        let mut nc = NoteClip::new(
            new_id(),
            String::new(),
            start_tick,
            (end_tick - start_tick).max(1),
        );
        // A clip with no window grows to its last note.
        if window.is_none() && c.hi >= i64::MAX / 8 {
            nc.len = 0;
        }
        nc
    }

    fn push_note_clip(&mut self, t: usize, mut nc: NoteClip) {
        if nc.len <= 0 {
            let end = nc.notes.iter().map(Note::end).max().unwrap_or(PPQ);
            nc.len = (end + PPQ - 1) / PPQ * PPQ;
        }
        if nc.notes.is_empty() && nc.controllers.iter().all(|l| l.points.is_empty()) {
            return;
        }
        let tr = &mut self.p.tracks[t];
        if tr.kind == TrackKind::Audio && tr.clips.is_empty() {
            tr.kind = TrackKind::Instrument;
        }
        self.p.tracks[t].note_clips.push(nc);
    }

    /// A tick inside `c` as ticks from the start of `nc`.
    fn rel(&self, c: &Ctx, at: f64, nc: &NoteClip) -> i64 {
        self.ctx_tick(c, at) - nc.start
    }

    fn notes(&mut self, n: Node, c: &Ctx, window: Option<(f64, f64)>, nc: &mut NoteClip) {
        for note in children(n, "Note") {
            let (Some(at), Some(dur), Some(key)) = (
                attr_num(note, "time"),
                attr_num(note, "duration"),
                attr_num(note, "key"),
            ) else {
                continue;
            };
            if let Some((a, b)) = window
                && (at < a - 1e-9 || at >= b - 1e-9)
            {
                continue;
            }
            let frame = self.ctx_frame(c, at);
            if frame < c.lo || frame >= c.hi {
                continue;
            }
            let start = self.rel(c, at, nc);
            let len = (self.ctx_tick(c, at + dur) - self.ctx_tick(c, at)).max(1);
            let vel = attr_num(note, "vel").unwrap_or(0.787);
            let mut out = Note::new(
                start,
                len,
                key.clamp(0.0, 127.0) as u8,
                (vel * 127.0).round().clamp(1.0, 127.0) as u8,
            );
            out.channel = attr_num(note, "channel").unwrap_or(0.0).clamp(0.0, 15.0) as u8;
            // Per-note expression: times from the note's start.
            let inner = self.inner(c, at, c.seconds);
            for pts in note
                .descendants()
                .filter(|d| d.tag_name().name() == "Points")
            {
                let expr = child(pts, "Target").and_then(|tg| tg.attribute("expression"));
                let kind = match expr {
                    Some("transpose") => Note::PITCH,
                    Some("pressure") | Some("polyPressure") => Note::PRESSURE,
                    Some("timbre") => Note::TIMBRE,
                    other => {
                        self.report.add(
                            "Notes",
                            format!(
                                "Per-note {} lines are left out.",
                                other.unwrap_or("unknown")
                            ),
                        );
                        continue;
                    }
                };
                let pc = Reader::unit(pts, &inner);
                let lane = out.expression_mut(kind);
                for p in pts
                    .children()
                    .filter(|p| p.tag_name().name() == "RealPoint")
                {
                    let (Some(pt), Some(v)) = (attr_num(p, "time"), attr_num(p, "value")) else {
                        continue;
                    };
                    let tick = self.ctx_tick(&pc, pt) - self.ctx_tick(&inner, 0.0);
                    let v = if kind == Note::PITCH {
                        v.clamp(-48.0, 48.0)
                    } else {
                        v.clamp(0.0, 1.0)
                    };
                    lane.put(tick, v);
                }
            }
            nc.notes.push(out);
        }
    }

    /// A controller line inside a note clip.
    fn controller(&mut self, pts: Node, c: &Ctx, window: Option<(f64, f64)>, nc: &mut NoteClip) {
        let Some(tg) = child(pts, "Target") else {
            return;
        };
        let (ctl, scale): (Controller, fn(f64) -> f64) = match tg.attribute("expression") {
            Some("channelController") => {
                let Some(n) = attr_num(tg, "controller") else {
                    return;
                };
                (Controller::Cc(n.clamp(0.0, 127.0) as u8), |v| {
                    (v * 127.0).clamp(0.0, 127.0)
                })
            }
            Some("pitchBend") => (Controller::PitchBend, |v| {
                (v * 16383.0 - 8192.0).clamp(-8192.0, 8191.0)
            }),
            Some("channelPressure") => (Controller::Pressure, |v| (v * 127.0).clamp(0.0, 127.0)),
            other => {
                self.report.add(
                    "Notes",
                    format!(
                        "{} lines in clips are left out.",
                        other.unwrap_or("Unknown")
                    ),
                );
                return;
            }
        };
        let points: Vec<(f64, f64, bool)> = point_list(pts);
        let start = nc.start;
        let lane = nc.controller_mut(ctl);
        let mut last: Option<(i64, f64, bool)> = None;
        for (at, v, hold) in points {
            if let Some((a, b)) = window
                && (at < a - 1e-9 || at > b + 1e-9)
            {
                continue;
            }
            let tick = self.ctx_tick(c, at) - start;
            let v = scale(v);
            if let Some((_, pv, true)) = last
                && tick > 0
            {
                lane.put(tick - 1, pv);
            }
            lane.put(tick, v);
            last = Some((tick, v, hold));
        }
    }

    /// Audio straight from a file, its time 0 at the file's start.
    fn audio(
        &mut self,
        n: Node,
        t: usize,
        c: &Ctx,
        window: Option<(f64, f64)>,
        warp: Option<&dyn Fn(f64) -> f64>,
    ) {
        let Some(f) = child(n, "File") else {
            return;
        };
        let Some(path) = self.audio_file(f) else {
            self.report.add(
                "Audio",
                format!(
                    "{} isn't in the file or beside it, so its clips are left out.",
                    f.attribute("path").unwrap_or("A sound file")
                ),
            );
            return;
        };
        let file_seconds = attr_num(n, "duration");
        let rate = self.p.rate;
        let (a, b) = window.unwrap_or((0.0, file_seconds.unwrap_or(0.0)));
        let start = self.ctx_frame(c, a).max(c.lo);
        let end = self.ctx_frame(c, b).min(c.hi);
        if end <= start {
            return;
        }
        // How far into the file the audio at `start` is.
        let at_start = if let Some(w) = warp {
            // In the warp's outer time.
            let outer = if c.seconds {
                time::frames_to_seconds(start - c.frame, rate)
            } else {
                frames_to_beats(&self.map, start) - c.beat
            };
            w(outer)
        } else if c.seconds {
            time::frames_to_seconds(start - c.frame, rate)
        } else {
            time::frames_to_seconds(start - self.ctx_frame(c, 0.0), rate)
        };
        let offset = time::seconds_to_frames(at_start.max(0.0), rate);
        let mut len = end - start;
        if let Some(s) = file_seconds {
            len = len.min(time::seconds_to_frames(s, rate) - offset).max(0);
        }
        if len <= 0 {
            return;
        }
        let name = Path::new(f.attribute("path").unwrap_or(""))
            .file_name()
            .map_or(String::new(), |n| n.to_string_lossy().into_owned());
        let tr = &mut self.p.tracks[t];
        let seq = tr.clips.iter().map(|c| c.seq).max().unwrap_or(0) + 1;
        tr.clips.push(Clip {
            id: new_id(),
            asset: path,
            name,
            start,
            offset,
            len,
            seq,
            ..Clip::default()
        });
    }

    /// Audio warped to the timeline. Our clips don't stretch yet, so it
    /// plays at its own speed from where the window starts.
    fn warps(&mut self, n: Node<'a, 'i>, t: usize, c: &Ctx, window: Option<(f64, f64)>) {
        let content_seconds = n.attribute("contentTimeUnit") != Some("beats");
        let mut pts: Vec<(f64, f64)> = children(n, "Warp")
            .filter_map(|w| Some((attr_num(w, "time")?, attr_num(w, "contentTime")?)))
            .collect();
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        let Some(audio) = n.children().find(|k| k.tag_name().name() == "Audio") else {
            self.report
                .add("Audio", "Warped content other than audio is left out.");
            return;
        };
        if pts.len() < 2 || !content_seconds {
            self.audio(audio, t, c, window, None);
            return;
        }
        let warp = move |x: f64| -> f64 {
            // Straight lines between the points, carried on past the ends.
            let i = pts.partition_point(|p| p.0 <= x).clamp(1, pts.len() - 1);
            let (a, b) = (pts[i - 1], pts[i]);
            if b.0 == a.0 {
                return a.1;
            }
            a.1 + (x - a.0) * (b.1 - a.1) / (b.0 - a.0)
        };
        // Does it play at its own speed? Compare the time the warp
        // gives the audio with the time it takes on the timeline.
        let (a, b) = window.unwrap_or((0.0, 1.0));
        let outer = (self.ctx_frame(c, b) - self.ctx_frame(c, a)) as f64 / self.p.rate as f64;
        let inner = warp(b) - warp(a);
        if outer > 0.0 && ((inner / outer) - 1.0).abs() > 0.005 {
            self.report.add(
                format!("Track \"{}\"", self.p.tracks[t].name),
                "Audio stretched to fit the tempo plays at its own speed here, from where it started.",
            );
        }
        self.audio(audio, t, c, window, Some(&warp));
    }

    /// Automation on a track.
    fn track_points(&mut self, pts: Node, track: Option<usize>, c: &Ctx) {
        let Some(tg) = child(pts, "Target") else {
            return;
        };
        let Some(param) = tg.attribute("parameter") else {
            let what = tg.attribute("expression").unwrap_or("controller");
            let name = track.map_or("Track".into(), |t| {
                format!("Track \"{}\"", self.p.tracks[t].name)
            });
            self.report.add(
                name,
                format!("{what} lines on the track itself are left out; in clips they come in."),
            );
            return;
        };
        let Some(target) = self.targets.get(param).cloned() else {
            self.report.add(
                "Automation",
                "Lines for settings that didn't come in are left out.",
            );
            return;
        };
        if target.param == "mute" || target.param.is_empty() {
            self.report.add(
                format!("Track \"{}\"", self.p.tracks[target.track].name),
                if target.param == "mute" {
                    "Mute automation is left out."
                } else {
                    "Automation of a send that didn't come in is left out."
                },
            );
            return;
        }
        let mut out: Vec<AutoPoint> = Vec::new();
        let mut last: Option<(i64, f64, bool)> = None;
        for (at, v, hold) in point_list(pts) {
            let frame = self.ctx_frame(c, at);
            if frame < c.lo || frame > c.hi {
                continue;
            }
            let v = target.conv.apply(v);
            // A held step: the value stays until just before the next.
            if let Some((pf, pv, true)) = last
                && frame - 1 > pf
            {
                out.push(AutoPoint {
                    at: frame - 1,
                    value: pv,
                });
            }
            out.push(AutoPoint {
                at: frame,
                value: v,
            });
            last = Some((frame, v, hold));
        }
        let tr = &mut self.p.tracks[target.track];
        let lane = match tr.automation.iter().position(|l| l.param == target.param) {
            Some(i) => &mut tr.automation[i],
            None => {
                tr.automation.push(Lane::new(target.param.clone()));
                tr.automation.last_mut().expect("just pushed")
            }
        };
        for p in out {
            lane.put(p.at, p.value);
        }
    }
}

/// A line's points: time, value, and whether it holds to the next.
fn point_list(pts: Node) -> Vec<(f64, f64, bool)> {
    let mut v: Vec<(f64, f64, bool)> = pts
        .children()
        .filter_map(|p| {
            let at = attr_num(p, "time")?;
            let value = match p.tag_name().name() {
                "RealPoint" | "IntegerPoint" | "EnumPoint" => attr_num(p, "value")?,
                "BoolPoint" => attr_bool(p, "value")? as u8 as f64,
                _ => return None,
            };
            // Real points hold unless they say linear; the others always.
            let hold = p.tag_name().name() != "RealPoint"
                || p.attribute("interpolation") != Some("linear");
            Some((at, value, hold))
        })
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v
}
