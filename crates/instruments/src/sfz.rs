// SPDX-License-Identifier: GPL-3.0-or-later
//! Reading SFZ files, the open text format most free sample libraries
//! use (specification: <https://sfzformat.com>). A file lists sound
//! files ("regions"), each with the keys, velocities and controller
//! values it plays on, how it is tuned, its level, its envelope, and
//! what it is grouped and choked with. Headers (`<global>`, `<master>`,
//! `<group>`) give settings to every region under them.
//!
//! This reads the text into [`Sfz`]: every region with its settings
//! resolved from the headers above it, the controller curves, the
//! controllers' names and starting values. Nothing here reads a sound;
//! [`crate::library`] does that. Opcodes this version doesn't play are
//! counted in [`Sfz::unsupported`], so the window can say what is left
//! out rather than play something else quietly.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// How a region's sound plays out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopMode {
    /// Once, stopping (with the release) when the key is let go.
    NoLoop,
    /// Once, to the end, whatever the key does.
    OneShot,
    /// Round the loop for as long as the voice sounds.
    Continuous,
    /// Round the loop while the key is held, then on to the end.
    Sustain,
}

/// What starts a region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Attack,
    /// When the key is let go (a damper falling, a string's release).
    Release,
    /// Only when no other key is held.
    First,
    /// Only when another key is held.
    Legato,
}

/// How a choked voice stops.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OffMode {
    /// At once, over a few milliseconds.
    Fast,
    /// With its own release.
    Normal,
    /// Over this many seconds.
    Time(f32),
}

/// What a controller moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    /// Multiplies the level: percent of full.
    Amplitude,
    /// Adds decibels.
    Volume,
    /// Adds to the pan, −100 to 100.
    Pan,
    /// Adds cents.
    Tune,
    /// Adds cents to the first filter's cutoff.
    Cutoff,
    Cutoff2,
    /// Adds seconds (or percent, for sustain and start) to the envelope.
    EgDelay,
    EgStart,
    EgAttack,
    EgHold,
    EgDecay,
    EgSustain,
    EgRelease,
    /// Adds seconds before the voice starts.
    Delay,
    /// Adds frames to where the sound starts.
    Offset,
}

/// A controller's effect on one setting: `amount` times the curve's
/// value at the controller's position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mod {
    pub target: Target,
    pub cc: u8,
    pub amount: f32,
    pub curve: u16,
}

/// The kind of a region's filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterKind {
    Low,
    High,
    Band,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Filter {
    pub kind: FilterKind,
    pub cutoff: f32,
    /// Decibels.
    pub resonance: f32,
    /// Cents at full velocity.
    pub veltrack: f32,
    /// Cents per key from `keycenter`.
    pub keytrack: f32,
    pub keycenter: u8,
}

/// A crossfade in or out over a range of key, velocity or a controller.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fade {
    pub lo: f32,
    pub hi: f32,
    /// Fading in (rising over the range) or out.
    pub rising: bool,
    /// Equal power (true) or equal gain.
    pub power: bool,
}

impl Fade {
    /// The gain at `v`.
    pub fn gain(&self, v: f32) -> f32 {
        let t = if self.hi <= self.lo {
            if v >= self.hi { 1.0 } else { 0.0 }
        } else {
            ((v - self.lo) / (self.hi - self.lo)).clamp(0.0, 1.0)
        };
        let t = if self.rising { t } else { 1.0 - t };
        if self.power { t.sqrt() } else { t }
    }
}

/// The amplitude envelope.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Eg {
    pub delay: f32,
    /// Percent of full that the attack starts from.
    pub start: f32,
    pub attack: f32,
    pub hold: f32,
    pub decay: f32,
    /// Percent.
    pub sustain: f32,
    pub release: f32,
    /// Seconds added at full velocity (`ampeg_vel2*`).
    pub vel2attack: f32,
    pub vel2hold: f32,
    pub vel2decay: f32,
    pub vel2sustain: f32,
    pub vel2release: f32,
}

impl Default for Eg {
    fn default() -> Eg {
        Eg {
            delay: 0.0,
            start: 0.0,
            attack: 0.0,
            hold: 0.0,
            decay: 0.0,
            sustain: 100.0,
            release: 0.001,
            vel2attack: 0.0,
            vel2hold: 0.0,
            vel2decay: 0.0,
            vel2sustain: 0.0,
            vel2release: 0.0,
        }
    }
}

/// One region: a sound and when and how it plays.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    /// The sound file's full path, or a sound made here (`*sine`,
    /// `*noise`, `*silence`).
    pub sample: String,
    /// The keys it plays on; `None` for none (a key-switch, or a region
    /// started by a controller).
    pub keys: Option<(u8, u8)>,
    pub vel: (u8, u8),
    /// MIDI channels, 1 to 16.
    pub chan: (u8, u8),
    /// Controller ranges it plays in: (controller, lowest, highest).
    pub cc: Vec<(u8, f32, f32)>,
    /// A random number from 0 to 1 at each note picks the regions whose
    /// range it falls in.
    pub rand: (f32, f32),
    /// Round robin: of every `seq_length` notes that would play it, it
    /// plays the `seq_position`th (from 1).
    pub seq_length: u32,
    pub seq_position: u32,
    pub trigger: Trigger,
    /// Started by a controller moving into this range: (controller,
    /// lowest, highest).
    pub on_cc: Option<(u8, f32, f32)>,
    /// Decibels a release region loses for each second the key was held.
    pub rt_decay: f32,
    pub keycenter: u8,
    /// Cents per key.
    pub keytrack: f32,
    pub transpose: i32,
    pub tune: f32,
    /// Cents at the wheel's full travel (bend_down is negative).
    pub bend_up: f32,
    pub bend_down: f32,
    /// Cents at full velocity.
    pub pitch_veltrack: f32,
    /// Cents up and down at random.
    pub pitch_random: f32,
    pub volume: f32,
    /// Percent.
    pub amplitude: f32,
    pub pan: f32,
    /// Decibels per key from `amp_keycenter`.
    pub amp_keytrack: f32,
    pub amp_keycenter: u8,
    /// Decibels down at random.
    pub amp_random: f32,
    /// Percent.
    pub amp_veltrack: f32,
    /// The velocity curve's points (velocity, gain); none for the
    /// usual curve.
    pub velcurve: Vec<(u8, f32)>,
    pub offset: u64,
    pub offset_random: u64,
    /// The last frame, if not the sound's last.
    pub end: Option<u64>,
    pub reverse: bool,
    pub loop_mode: LoopMode,
    /// The loop's first and last frames, if set.
    pub loop_start: Option<u64>,
    pub loop_end: Option<u64>,
    /// Seconds before it starts, and up to this many more at random.
    pub delay: f32,
    pub delay_random: f32,
    pub eg: Eg,
    pub filters: [Option<Filter>; 2],
    pub mods: Vec<Mod>,
    pub xfades: Vec<(Fader, Fade)>,
    /// Voices of the same `group` that are stopped when a region whose
    /// group is `off_by` starts.
    pub group: i64,
    pub off_by: Option<i64>,
    pub off_mode: OffMode,
    /// Voices at most in its group, and for one key in its group.
    pub polyphony: Option<u32>,
    pub note_polyphony: Option<u32>,
    /// Key-switches: the range of keys that switch, and the one (or
    /// range) whose last press lets this region play.
    pub sw_range: Option<(u8, u8)>,
    pub sw_last: Option<(u8, u8)>,
    /// Plays only while this key is held, or isn't.
    pub sw_down: Option<u8>,
    pub sw_up: Option<u8>,
    /// Plays only if the note before was this key.
    pub sw_previous: Option<u8>,
    pub sw_label: Option<String>,
    /// Its name, from `region_label`, or its group's, master's or
    /// global's label.
    pub label: Option<String>,
    /// The sustain pedal's controller.
    pub sustain_cc: u8,
}

/// What a crossfade follows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fader {
    Key,
    Velocity,
    Cc(u8),
}

impl Default for Region {
    fn default() -> Region {
        Region {
            sample: String::new(),
            keys: Some((0, 127)),
            vel: (1, 127),
            chan: (1, 16),
            cc: Vec::new(),
            rand: (0.0, 1.0),
            seq_length: 1,
            seq_position: 1,
            trigger: Trigger::Attack,
            on_cc: None,
            rt_decay: 0.0,
            keycenter: 60,
            keytrack: 100.0,
            transpose: 0,
            tune: 0.0,
            bend_up: 200.0,
            bend_down: -200.0,
            pitch_veltrack: 0.0,
            pitch_random: 0.0,
            volume: 0.0,
            amplitude: 100.0,
            pan: 0.0,
            amp_keytrack: 0.0,
            amp_keycenter: 60,
            amp_random: 0.0,
            amp_veltrack: 100.0,
            velcurve: Vec::new(),
            offset: 0,
            offset_random: 0,
            end: None,
            reverse: false,
            loop_mode: LoopMode::NoLoop,
            loop_start: None,
            loop_end: None,
            delay: 0.0,
            delay_random: 0.0,
            eg: Eg::default(),
            filters: [None, None],
            mods: Vec::new(),
            xfades: Vec::new(),
            group: 0,
            off_by: None,
            off_mode: OffMode::Fast,
            polyphony: None,
            note_polyphony: None,
            sw_range: None,
            sw_last: None,
            sw_down: None,
            sw_up: None,
            sw_previous: None,
            sw_label: None,
            label: None,
            sustain_cc: 64,
        }
    }
}

impl Region {
    /// The gain at `velocity`, from the velocity curve and how much the
    /// region follows velocity.
    pub fn velocity_gain(&self, velocity: u8) -> f32 {
        let track = (self.amp_veltrack / 100.0).clamp(-1.0, 1.0);
        let v = if track < 0.0 {
            127 - velocity.min(127)
        } else {
            velocity.min(127)
        };
        let c = if self.velcurve.is_empty() {
            let x = v as f32 / 127.0;
            x * x
        } else {
            curve_points(&self.velcurve, v)
        };
        let t = track.abs();
        1.0 - t + t * c
    }

    /// Whether `key` (and `velocity`) fall in its ranges.
    pub fn plays(&self, key: u8, velocity: u8) -> bool {
        self.keys.is_some_and(|(lo, hi)| (lo..=hi).contains(&key))
            && (self.vel.0..=self.vel.1).contains(&velocity)
    }
}

/// Linear between the points given, with 0 at 0 and 1 at 127 unless
/// they are given.
fn curve_points(points: &[(u8, f32)], v: u8) -> f32 {
    let mut pts: Vec<(u8, f32)> = Vec::with_capacity(points.len() + 2);
    if !points.iter().any(|p| p.0 == 0) {
        pts.push((0, 0.0));
    }
    pts.extend_from_slice(points);
    if !points.iter().any(|p| p.0 == 127) {
        pts.push((127, 1.0));
    }
    pts.sort_by_key(|p| p.0);
    interpolate(&pts, v)
}

fn interpolate(pts: &[(u8, f32)], v: u8) -> f32 {
    let mut prev = pts[0];
    if v <= prev.0 {
        return prev.1;
    }
    for &p in &pts[1..] {
        if v <= p.0 {
            let span = (p.0 - prev.0) as f32;
            let t = if span > 0.0 {
                (v - prev.0) as f32 / span
            } else {
                1.0
            };
            return prev.1 + (p.1 - prev.1) * t;
        }
        prev = p;
    }
    prev.1
}

/// A controller curve: a value for each position, 0 to 127.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve(pub [f32; 128]);

impl Curve {
    /// The curves every SFZ player has, 0 to 6.
    pub fn builtin(i: u16) -> Option<Curve> {
        let f: fn(f32) -> f32 = match i {
            0 => |x| x,
            1 => |x| 2.0 * x - 1.0,
            2 => |x| 1.0 - x,
            3 => |x| 1.0 - 2.0 * x,
            4 => |x| x * x,
            5 => |x: f32| x.sqrt(),
            6 => |x: f32| (1.0 - x).sqrt(),
            _ => return None,
        };
        let mut c = [0.0; 128];
        for (v, slot) in c.iter_mut().enumerate() {
            *slot = f(v as f32 / 127.0);
        }
        Some(Curve(c))
    }

    fn from_points(points: &[(u8, f32)]) -> Curve {
        let mut c = [0.0; 128];
        for (v, slot) in c.iter_mut().enumerate() {
            *slot = curve_points(points, v as u8);
        }
        Curve(c)
    }

    /// The value at controller position `v` (0 to 127, between steps
    /// too).
    pub fn at(&self, v: f32) -> f32 {
        let v = v.clamp(0.0, 127.0);
        let i = v as usize;
        if i >= 127 {
            return self.0[127];
        }
        let t = v - i as f32;
        self.0[i] + (self.0[i + 1] - self.0[i]) * t
    }
}

/// A whole SFZ instrument.
#[derive(Clone, Debug, Default)]
pub struct Sfz {
    pub regions: Vec<Region>,
    /// Curves by index, the built-in ones and the file's own.
    pub curves: BTreeMap<u16, Curve>,
    /// Controllers' names, as the file gives them.
    pub cc_labels: BTreeMap<u8, String>,
    /// Controllers' starting positions, 0 to 127, as the file sets them.
    pub cc_start: BTreeMap<u8, f32>,
    /// Keys' names (`label_keyN`), and key-switches' (`sw_label`).
    pub key_labels: BTreeMap<u8, String>,
    /// The key-switch first in effect.
    pub sw_default: Option<u8>,
    /// Opcodes this version doesn't play, with how many times each is
    /// used.
    pub unsupported: BTreeMap<String, usize>,
    /// Files that couldn't be read, and other problems.
    pub problems: Vec<String>,
}

impl Sfz {
    /// The value of curve `i` at controller position `v`; the straight
    /// line for a curve that isn't there.
    pub fn curve(&self, i: u16, v: f32) -> f32 {
        match self.curves.get(&i) {
            Some(c) => c.at(v),
            None => v.clamp(0.0, 127.0) / 127.0,
        }
    }

    /// Every controller the instrument names or listens to, besides the
    /// sustain pedal, in order.
    pub fn controllers(&self) -> Vec<u8> {
        let mut v: Vec<u8> = self.cc_labels.keys().copied().collect();
        v.extend(self.cc_start.keys().copied());
        for r in &self.regions {
            v.extend(r.mods.iter().map(|m| m.cc));
            v.extend(r.cc.iter().map(|c| c.0));
            v.extend(r.on_cc.iter().map(|c| c.0));
            v.extend(r.xfades.iter().filter_map(|(f, _)| match f {
                Fader::Cc(c) => Some(*c),
                _ => None,
            }));
        }
        v.retain(|&c| c < 128 && c != 64);
        v.sort_unstable();
        v.dedup();
        v
    }

    /// The keys that switch between articulations, with their names.
    pub fn key_switches(&self) -> Vec<(u8, String)> {
        let mut keys: BTreeMap<u8, String> = BTreeMap::new();
        for r in &self.regions {
            if let Some((lo, hi)) = r.sw_last {
                for k in lo..=hi {
                    let name = (self.key_labels.get(&k).cloned())
                        .or_else(|| r.sw_label.clone())
                        .unwrap_or_else(|| crate::params::note_name(k as i32));
                    keys.entry(k).or_insert(name);
                }
            }
        }
        keys.into_iter().collect()
    }
}

/// Reads the SFZ file at `path`, with what it includes.
pub fn read(path: &Path) -> Result<Sfz, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let read_include = |p: &Path| std::fs::read_to_string(p).map_err(|e| e.to_string());
    Ok(parse(&text, &dir, &read_include))
}

/// Reads SFZ text whose sounds and includes are found from `dir`, the
/// folder of the file it came from; `include` reads an included file.
pub fn parse(text: &str, dir: &Path, include: &dyn Fn(&Path) -> Result<String, String>) -> Sfz {
    let mut sfz = Sfz::default();
    let mut defines: Vec<(String, String)> = Vec::new();
    let mut flat = String::new();
    expand(
        text,
        dir,
        include,
        &mut defines,
        &mut flat,
        0,
        &mut sfz.problems,
    );
    build(&tokens(&flat), dir, &mut sfz);
    sfz
}

/// Strips comments, follows `#include`s and puts `#define`d values in.
fn expand(
    text: &str,
    dir: &Path,
    include: &dyn Fn(&Path) -> Result<String, String>,
    defines: &mut Vec<(String, String)>,
    out: &mut String,
    depth: usize,
    problems: &mut Vec<String>,
) {
    let text = strip_block_comments(text);
    for line in text.lines() {
        let line = match line.find("//") {
            Some(i) => &line[..i],
            None => line,
        };
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("#define") {
            let mut parts = rest.split_whitespace();
            if let (Some(name), Some(value)) = (parts.next(), parts.next()) {
                let name = name.trim_start_matches('$').to_string();
                let value = substitute(value, defines);
                defines.retain(|(n, _)| *n != name);
                defines.push((name, value));
                // Longest first, so `$a_b` isn't read as `$a` and `_b`.
                defines.sort_by_key(|d| std::cmp::Reverse(d.0.len()));
            }
            continue;
        }
        if let Some(rest) = t.strip_prefix("#include") {
            let rest = substitute(rest.trim(), defines);
            let name = rest.trim().trim_matches('"').replace('\\', "/");
            if depth >= 16 {
                problems.push(format!("{name}: included too deep"));
                continue;
            }
            let p = dir.join(&name);
            match include(&p) {
                Ok(t) => expand(&t, dir, include, defines, out, depth + 1, problems),
                Err(e) => problems.push(format!("{name}: {e}")),
            }
            continue;
        }
        out.push_str(&substitute(line, defines));
        out.push('\n');
    }
}

fn strip_block_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        match rest[i + 2..].find("*/") {
            Some(j) => rest = &rest[i + 2 + j + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

fn substitute(s: &str, defines: &[(String, String)]) -> String {
    if !s.contains('$') {
        return s.to_string();
    }
    let mut out = s.to_string();
    for (n, v) in defines {
        out = out.replace(&format!("${n}"), v);
    }
    out
}

#[derive(Debug, PartialEq)]
enum Token {
    Header(String),
    Op(String, String),
}

/// Opcodes whose values may have spaces in them.
fn spaced(name: &str) -> bool {
    name == "sample"
        || name == "default_path"
        || name.ends_with("label")
        || name.starts_with("label_")
}

fn tokens(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    for line in text.lines() {
        let b = line.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i].is_ascii_whitespace() {
                i += 1;
                continue;
            }
            if b[i] == b'<' {
                let Some(j) = line[i..].find('>') else {
                    break;
                };
                out.push(Token::Header(
                    line[i + 1..i + j].trim().to_ascii_lowercase(),
                ));
                i += j + 1;
                continue;
            }
            // An opcode: a name, `=`, and a value.
            let Some(eq) = line[i..].find('=') else {
                break;
            };
            let name = line[i..i + eq].trim().to_string();
            let start = i + eq + 1;
            let mut end = line[start..].find('<').map_or(b.len(), |k| start + k);
            if spaced(&name) {
                // To the next ` name=`.
                if let Some(k) = next_opcode(&line[start..end]) {
                    end = start + k;
                }
            } else if let Some(k) = line[start..end].find(char::is_whitespace) {
                // The value ends at the first space; skip to the next
                // word.
                end = start + k;
            }
            let value = line[start..end].trim().to_string();
            if !name.is_empty() && !name.contains(char::is_whitespace) {
                out.push(Token::Op(name, value));
            }
            i = end;
        }
    }
    out
}

/// Where the next ` name=` starts in `s`, if anywhere.
fn next_opcode(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            let w = i;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            let n = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            if i > n && i < b.len() && b[i] == b'=' {
                return Some(w);
            }
        } else {
            i += 1;
        }
    }
    None
}

type Ops = BTreeMap<String, String>;

/// Turns the tokens into regions, each with every header's opcodes
/// above it.
fn build(tokens: &[Token], dir: &Path, sfz: &mut Sfz) {
    #[derive(PartialEq, Clone, Copy)]
    enum At {
        None,
        Control,
        Global,
        Master,
        Group,
        Region,
        Curve,
        Other,
    }
    let mut at = At::None;
    let (mut control, mut global, mut master, mut group, mut region) =
        (Ops::new(), Ops::new(), Ops::new(), Ops::new(), Ops::new());
    let mut curve: Ops = Ops::new();
    let mut regions: Vec<Ops> = Vec::new();
    let mut curves: Vec<Ops> = Vec::new();
    let flush = |at: At,
                 region: &mut Ops,
                 curve: &mut Ops,
                 regions: &mut Vec<Ops>,
                 curves: &mut Vec<Ops>,
                 layers: [&Ops; 3]| {
        match at {
            At::Region => {
                let mut all = Ops::new();
                for l in layers {
                    all.extend(l.iter().map(|(k, v)| (k.clone(), v.clone())));
                }
                all.append(region);
                regions.push(all);
            }
            At::Curve => curves.push(std::mem::take(curve)),
            _ => {}
        }
    };
    for t in tokens {
        match t {
            Token::Header(h) => {
                flush(
                    at,
                    &mut region,
                    &mut curve,
                    &mut regions,
                    &mut curves,
                    [&global, &master, &group],
                );
                region.clear();
                at = match h.as_str() {
                    "control" => At::Control,
                    "global" => {
                        global.clear();
                        master.clear();
                        group.clear();
                        At::Global
                    }
                    "master" => {
                        master.clear();
                        group.clear();
                        At::Master
                    }
                    "group" => {
                        group.clear();
                        At::Group
                    }
                    "region" => At::Region,
                    "curve" => At::Curve,
                    _ => At::Other,
                };
            }
            Token::Op(k, v) => {
                let target = match at {
                    At::Control => &mut control,
                    At::Global | At::None => &mut global,
                    At::Master => &mut master,
                    At::Group => &mut group,
                    At::Region => &mut region,
                    At::Curve => &mut curve,
                    At::Other => continue,
                };
                target.insert(k.clone(), v.clone());
            }
        }
    }
    flush(
        at,
        &mut region,
        &mut curve,
        &mut regions,
        &mut curves,
        [&global, &master, &group],
    );

    // The control header.
    let note_offset = int(control.get("note_offset")).unwrap_or(0)
        + 12 * int(control.get("octave_offset")).unwrap_or(0);
    let default_path = control
        .get("default_path")
        .map(|p| p.replace('\\', "/"))
        .unwrap_or_default();
    for (k, v) in &control {
        if let Some(n) = k
            .strip_prefix("label_cc")
            .and_then(|n| n.parse::<u8>().ok())
        {
            sfz.cc_labels.insert(n, v.clone());
        } else if let Some(n) = k.strip_prefix("set_cc").and_then(|n| n.parse::<u8>().ok()) {
            sfz.cc_start
                .insert(n, num(Some(v)).unwrap_or(0.0).clamp(0.0, 127.0) as f32);
        } else if let Some(n) = k
            .strip_prefix("set_hdcc")
            .and_then(|n| n.parse::<u8>().ok())
        {
            sfz.cc_start.insert(
                n,
                num(Some(v)).unwrap_or(0.0).clamp(0.0, 1.0) as f32 * 127.0,
            );
        } else if let Some(n) = k
            .strip_prefix("label_key")
            .and_then(|n| n.parse::<u8>().ok())
        {
            sfz.key_labels.insert(n, v.clone());
        } else if !matches!(
            k.as_str(),
            "default_path" | "note_offset" | "octave_offset" | "hint_ram_based" | "hint_stealing"
        ) && !k.starts_with("hint_")
        {
            *sfz.unsupported.entry(k.clone()).or_default() += 1;
        }
    }

    for i in 0..=6 {
        sfz.curves.insert(i, Curve::builtin(i).expect("built in"));
    }
    for c in &curves {
        let Some(i) = int(c.get("curve_index")).filter(|i| (7..=255).contains(i)) else {
            continue;
        };
        let pts: Vec<(u8, f32)> = (c.iter())
            .filter_map(|(k, v)| {
                let n = k
                    .strip_prefix('v')?
                    .parse::<u8>()
                    .ok()
                    .filter(|n| *n < 128)?;
                Some((n, num(Some(v))? as f32))
            })
            .collect();
        sfz.curves.insert(i as u16, Curve::from_points(&pts));
    }

    for ops in &regions {
        let r = region_of(ops, dir, &default_path, note_offset, sfz);
        if let Some(r) = r {
            if let Some(k) = r.sw_last.map(|s| s.0)
                && let Some(l) = &r.sw_label
            {
                sfz.key_labels.entry(k).or_insert_with(|| l.clone());
            }
            sfz.regions.push(r);
        }
    }
    sfz.sw_default = regions
        .iter()
        .find_map(|o| note(o.get("sw_default")).map(|n| (n + note_offset).clamp(0, 127) as u8));
}

fn num(v: Option<&String>) -> Option<f64> {
    v?.trim().parse::<f64>().ok().filter(|x| x.is_finite())
}

fn int(v: Option<&String>) -> Option<i64> {
    num(v).map(|x| x.round() as i64)
}

/// A key, as a number or a note name (`c4` is 60, `c#4` or `db4` 61).
pub fn note(v: Option<&String>) -> Option<i64> {
    let s = v?.trim();
    if let Ok(n) = s.parse::<i64>() {
        return Some(n);
    }
    let lower = s.to_ascii_lowercase();
    let b = lower.as_bytes();
    let base = match b.first()? {
        b'c' => 0,
        b'd' => 2,
        b'e' => 4,
        b'f' => 5,
        b'g' => 7,
        b'a' => 9,
        b'b' => 11,
        _ => return None,
    };
    let mut i = 1;
    let mut n = base;
    match b.get(1) {
        Some(b'#') => {
            n += 1;
            i = 2;
        }
        Some(b'b') if b.len() > 2 => {
            n -= 1;
            i = 2;
        }
        _ => {}
    }
    let octave: i64 = lower[i..].parse().ok()?;
    Some(n + (octave + 1) * 12)
}

/// The opcodes a region understands, so the rest can be counted as left
/// out.
fn known(k: &str) -> bool {
    const PLAIN: &[&str] = &[
        "sample",
        "key",
        "lokey",
        "hikey",
        "lovel",
        "hivel",
        "lochan",
        "hichan",
        "lorand",
        "hirand",
        "seq_length",
        "seq_position",
        "trigger",
        "rt_decay",
        "pitch_keycenter",
        "pitch_keytrack",
        "transpose",
        "tune",
        "pitch",
        "bend_up",
        "bend_down",
        "pitch_veltrack",
        "pitch_random",
        "volume",
        "amplitude",
        "pan",
        "amp_keytrack",
        "amp_keycenter",
        "amp_random",
        "amp_veltrack",
        "offset",
        "offset_random",
        "end",
        "direction",
        "loop_mode",
        "loopmode",
        "loop_start",
        "loopstart",
        "loop_end",
        "loopend",
        "delay",
        "delay_random",
        "group",
        "polyphony_group",
        "off_by",
        "off_mode",
        "off_time",
        "polyphony",
        "note_polyphony",
        "sw_lokey",
        "sw_hikey",
        "sw_last",
        "sw_lolast",
        "sw_hilast",
        "sw_down",
        "sw_up",
        "sw_previous",
        "sw_label",
        "sw_default",
        "fil_type",
        "filtype",
        "cutoff",
        "resonance",
        "fil_veltrack",
        "fil_keytrack",
        "fil_keycenter",
        "fil2_type",
        "cutoff2",
        "resonance2",
        "fil2_veltrack",
        "fil2_keytrack",
        "fil2_keycenter",
        "xfin_lovel",
        "xfin_hivel",
        "xfout_lovel",
        "xfout_hivel",
        "xfin_lokey",
        "xfin_hikey",
        "xfout_lokey",
        "xfout_hikey",
        "xf_velcurve",
        "xf_keycurve",
        "xf_cccurve",
        "global_volume",
        "master_volume",
        "group_volume",
        "global_amplitude",
        "master_amplitude",
        "group_amplitude",
        "sustain_cc",
        "region_label",
        "group_label",
        "master_label",
        "global_label",
        "ampeg_delay",
        "ampeg_start",
        "ampeg_attack",
        "ampeg_hold",
        "ampeg_decay",
        "ampeg_sustain",
        "ampeg_release",
        "ampeg_vel2attack",
        "ampeg_vel2hold",
        "ampeg_vel2decay",
        "ampeg_vel2sustain",
        "ampeg_vel2release",
    ];
    if PLAIN.contains(&k) || k.starts_with("amp_velcurve_") || k.starts_with("hint_") {
        return true;
    }
    cc_opcode(k).is_some()
}

/// An opcode that names a controller: its stem and the controller.
fn cc_opcode(k: &str) -> Option<(&str, u8)> {
    let digits = k.len() - k.bytes().rev().take_while(u8::is_ascii_digit).count();
    if digits == k.len() {
        return None;
    }
    let cc: u8 = k[digits..].parse().ok().filter(|n: &u8| *n < 128)?;
    let stem = &k[..digits];
    const STEMS: &[&str] = &[
        "locc",
        "hicc",
        "lohdcc",
        "hihdcc",
        "on_locc",
        "on_hicc",
        "xfin_locc",
        "xfin_hicc",
        "xfout_locc",
        "xfout_hicc",
    ];
    if STEMS.contains(&stem) {
        return Some((stem, cc));
    }
    let base = (stem.strip_suffix("_oncc"))
        .or_else(|| stem.strip_suffix("_curvecc"))
        .or_else(|| stem.strip_suffix("_cc"))
        .or_else(|| stem.strip_suffix("cc"))?;
    target(base).map(|_| (stem, cc))
}

fn target(base: &str) -> Option<Target> {
    Some(match base {
        "amplitude" => Target::Amplitude,
        "volume" | "gain" => Target::Volume,
        "pan" => Target::Pan,
        "tune" | "pitch" => Target::Tune,
        "cutoff" => Target::Cutoff,
        "cutoff2" => Target::Cutoff2,
        "ampeg_delay" => Target::EgDelay,
        "ampeg_start" => Target::EgStart,
        "ampeg_attack" => Target::EgAttack,
        "ampeg_hold" => Target::EgHold,
        "ampeg_decay" => Target::EgDecay,
        "ampeg_sustain" => Target::EgSustain,
        "ampeg_release" => Target::EgRelease,
        "delay" => Target::Delay,
        "offset" => Target::Offset,
        _ => return None,
    })
}

fn filter_of(ops: &Ops, n: &str) -> Option<Filter> {
    let f = |k: &str| ops.get(&format!("{}{k}", if n.is_empty() { "fil" } else { "fil2" }));
    let ty = if n.is_empty() {
        ops.get("fil_type").or_else(|| ops.get("filtype"))
    } else {
        ops.get("fil2_type")
    };
    let cutoff = num(ops.get(&format!("cutoff{n}")))? as f32;
    let kind = match ty.map(String::as_str).unwrap_or("lpf_2p") {
        t if t.starts_with("lpf") => FilterKind::Low,
        t if t.starts_with("hpf") => FilterKind::High,
        t if t.starts_with("bpf") => FilterKind::Band,
        _ => return None,
    };
    Some(Filter {
        kind,
        cutoff,
        resonance: num(ops.get(&format!("resonance{n}"))).unwrap_or(0.0) as f32,
        veltrack: num(f("_veltrack")).unwrap_or(0.0) as f32,
        keytrack: num(f("_keytrack")).unwrap_or(0.0) as f32,
        keycenter: note(f("_keycenter")).unwrap_or(60).clamp(0, 127) as u8,
    })
}

fn region_of(
    ops: &Ops,
    dir: &Path,
    default_path: &str,
    note_offset: i64,
    sfz: &mut Sfz,
) -> Option<Region> {
    let mut r = Region::default();
    let sample = ops.get("sample")?.replace('\\', "/");
    r.sample = if sample.starts_with('*') {
        sample
    } else {
        let p: PathBuf = dir.join(default_path).join(&sample);
        p.to_string_lossy().into_owned()
    };
    for k in ops.keys() {
        if !known(k) {
            *sfz.unsupported.entry(strip_number(k)).or_default() += 1;
        }
    }
    let key = |k: &str| note(ops.get(k)).map(|n| n + note_offset);
    let key8 = |k: &str| key(k).filter(|n| (0..=127).contains(n)).map(|n| n as u8);
    let f = |k: &str| num(ops.get(k)).map(|x| x as f32);

    // Keys: `key` sets all three; -1 for none.
    let mut lo = key("lokey");
    let mut hi = key("hikey");
    if let Some(k) = key("key") {
        lo = lo.or(Some(k));
        hi = hi.or(Some(k));
        r.keycenter = k.clamp(0, 127) as u8;
    }
    if let Some(c) = key("pitch_keycenter") {
        r.keycenter = c.clamp(0, 127) as u8;
    }
    let (lo, hi) = (lo.unwrap_or(0), hi.unwrap_or(127));
    r.keys = (lo >= 0 && hi >= 0 && lo <= 127 && lo <= hi)
        .then(|| (lo.clamp(0, 127) as u8, hi.clamp(0, 127) as u8));
    let v8 = |k: &str, d: u8| int(ops.get(k)).map_or(d, |v| v.clamp(0, 127) as u8);
    r.vel = (v8("lovel", 1).max(1), v8("hivel", 127));
    r.chan = (
        int(ops.get("lochan")).map_or(1, |v| v.clamp(1, 16) as u8),
        int(ops.get("hichan")).map_or(16, |v| v.clamp(1, 16) as u8),
    );
    r.rand = (f("lorand").unwrap_or(0.0), f("hirand").unwrap_or(1.0));
    r.seq_length = int(ops.get("seq_length")).map_or(1, |v| v.max(1) as u32);
    r.seq_position = int(ops.get("seq_position")).map_or(1, |v| v.max(1) as u32);
    r.trigger = match ops.get("trigger").map(String::as_str) {
        Some("release") | Some("release_key") => Trigger::Release,
        Some("first") => Trigger::First,
        Some("legato") => Trigger::Legato,
        _ => Trigger::Attack,
    };
    r.rt_decay = f("rt_decay").unwrap_or(0.0);
    r.keytrack = f("pitch_keytrack").unwrap_or(100.0);
    r.transpose = int(ops.get("transpose")).unwrap_or(0) as i32;
    r.tune = f("tune").or_else(|| f("pitch")).unwrap_or(0.0);
    r.bend_up = f("bend_up").unwrap_or(200.0);
    r.bend_down = f("bend_down").unwrap_or(-200.0);
    r.pitch_veltrack = f("pitch_veltrack").unwrap_or(0.0);
    r.pitch_random = f("pitch_random").unwrap_or(0.0);
    r.volume = f("volume").unwrap_or(0.0)
        + f("global_volume").unwrap_or(0.0)
        + f("master_volume").unwrap_or(0.0)
        + f("group_volume").unwrap_or(0.0);
    r.amplitude = f("amplitude").unwrap_or(100.0) * f("global_amplitude").unwrap_or(100.0) / 100.0
        * f("master_amplitude").unwrap_or(100.0)
        / 100.0
        * f("group_amplitude").unwrap_or(100.0)
        / 100.0;
    r.pan = f("pan").unwrap_or(0.0);
    r.amp_keytrack = f("amp_keytrack").unwrap_or(0.0);
    r.amp_keycenter = key8("amp_keycenter").unwrap_or(60);
    r.amp_random = f("amp_random").unwrap_or(0.0);
    r.amp_veltrack = f("amp_veltrack").unwrap_or(100.0);
    r.velcurve = (ops.iter())
        .filter_map(|(k, v)| {
            let n = k
                .strip_prefix("amp_velcurve_")?
                .parse::<u8>()
                .ok()
                .filter(|n| *n < 128)?;
            Some((n, num(Some(v))? as f32))
        })
        .collect();
    r.offset = int(ops.get("offset")).map_or(0, |v| v.max(0) as u64);
    r.offset_random = int(ops.get("offset_random")).map_or(0, |v| v.max(0) as u64);
    r.end = int(ops.get("end")).filter(|v| *v > 0).map(|v| v as u64);
    r.reverse = ops.get("direction").is_some_and(|d| d == "reverse");
    r.loop_mode = match ops
        .get("loop_mode")
        .or_else(|| ops.get("loopmode"))
        .map(String::as_str)
    {
        Some("one_shot") => LoopMode::OneShot,
        Some("loop_continuous") => LoopMode::Continuous,
        Some("loop_sustain") => LoopMode::Sustain,
        Some("no_loop") => LoopMode::NoLoop,
        _ if r.sample.starts_with('*') => LoopMode::Continuous,
        _ => LoopMode::NoLoop,
    };
    let frame = |a: &str, b: &str| {
        int(ops.get(a).or_else(|| ops.get(b)))
            .filter(|v| *v >= 0)
            .map(|v| v as u64)
    };
    r.loop_start = frame("loop_start", "loopstart");
    r.loop_end = frame("loop_end", "loopend");
    r.delay = f("delay").unwrap_or(0.0);
    r.delay_random = f("delay_random").unwrap_or(0.0);
    let eg = &mut r.eg;
    let g = |k: &str, d: f32| f(&format!("ampeg_{k}")).unwrap_or(d);
    *eg = Eg {
        delay: g("delay", 0.0),
        start: g("start", 0.0),
        attack: g("attack", 0.0),
        hold: g("hold", 0.0),
        decay: g("decay", 0.0),
        sustain: g("sustain", 100.0),
        release: g("release", 0.001),
        vel2attack: g("vel2attack", 0.0),
        vel2hold: g("vel2hold", 0.0),
        vel2decay: g("vel2decay", 0.0),
        vel2sustain: g("vel2sustain", 0.0),
        vel2release: g("vel2release", 0.0),
    };
    r.filters = [filter_of(ops, ""), filter_of(ops, "2")];
    r.group = int(ops.get("group").or_else(|| ops.get("polyphony_group"))).unwrap_or(0);
    r.off_by = int(ops.get("off_by")).filter(|g| *g != 0);
    r.off_mode = match ops.get("off_mode").map(String::as_str) {
        Some("normal") => OffMode::Normal,
        Some("time") => OffMode::Time(f("off_time").unwrap_or(0.006)),
        _ => OffMode::Fast,
    };
    r.polyphony = int(ops.get("polyphony"))
        .filter(|v| *v > 0)
        .map(|v| v as u32);
    r.note_polyphony = int(ops.get("note_polyphony"))
        .filter(|v| *v > 0)
        .map(|v| v as u32);
    if let (Some(a), Some(b)) = (key8("sw_lokey"), key8("sw_hikey")) {
        r.sw_range = Some((a.min(b), a.max(b)));
    }
    if let Some(k) = key8("sw_last") {
        r.sw_last = Some((k, k));
    } else if let (Some(a), Some(b)) = (key8("sw_lolast"), key8("sw_hilast")) {
        r.sw_last = Some((a.min(b), a.max(b)));
    }
    r.sw_down = key8("sw_down");
    r.sw_up = key8("sw_up");
    r.sw_previous = key8("sw_previous");
    r.sw_label = ops.get("sw_label").cloned();
    r.label = [
        "region_label",
        "group_label",
        "master_label",
        "global_label",
    ]
    .iter()
    .find_map(|k| ops.get(*k).filter(|v| !v.trim().is_empty()).cloned());
    r.sustain_cc = int(ops.get("sustain_cc")).map_or(64, |v| v.clamp(0, 127) as u8);

    // Crossfades.
    let power = |k: &str| ops.get(k).is_none_or(|c| c != "gain");
    let mut fade = |fader: Fader, lo: Option<f32>, hi: Option<f32>, rising: bool, power: bool| {
        if lo.is_some() || hi.is_some() {
            let (lo, hi) = (lo.unwrap_or(0.0), hi.unwrap_or(127.0));
            r.xfades.push((
                fader,
                Fade {
                    lo,
                    hi,
                    rising,
                    power,
                },
            ));
        }
    };
    let kf = |k: &str| key(k).map(|n| n as f32);
    fade(
        Fader::Velocity,
        f("xfin_lovel"),
        f("xfin_hivel"),
        true,
        power("xf_velcurve"),
    );
    fade(
        Fader::Velocity,
        f("xfout_lovel"),
        f("xfout_hivel"),
        false,
        power("xf_velcurve"),
    );
    fade(
        Fader::Key,
        kf("xfin_lokey"),
        kf("xfin_hikey"),
        true,
        power("xf_keycurve"),
    );
    fade(
        Fader::Key,
        kf("xfout_lokey"),
        kf("xfout_hikey"),
        false,
        power("xf_keycurve"),
    );

    // Controllers: ranges, crossfades and what they move.
    let mut ranges: BTreeMap<u8, (f32, f32)> = BTreeMap::new();
    let mut on: BTreeMap<u8, (f32, f32)> = BTreeMap::new();
    let mut xf: BTreeMap<(u8, bool), (Option<f32>, Option<f32>)> = BTreeMap::new();
    for (k, v) in ops {
        let Some((stem, cc)) = cc_opcode(k) else {
            continue;
        };
        let Some(x) = num(Some(v)).map(|x| x as f32) else {
            continue;
        };
        match stem {
            "locc" => ranges.entry(cc).or_insert((0.0, 127.0)).0 = x,
            "hicc" => ranges.entry(cc).or_insert((0.0, 127.0)).1 = x,
            "lohdcc" => ranges.entry(cc).or_insert((0.0, 127.0)).0 = x * 127.0,
            "hihdcc" => ranges.entry(cc).or_insert((0.0, 127.0)).1 = x * 127.0,
            "on_locc" => on.entry(cc).or_insert((0.0, 127.0)).0 = x,
            "on_hicc" => on.entry(cc).or_insert((0.0, 127.0)).1 = x,
            "xfin_locc" => xf.entry((cc, true)).or_default().0 = Some(x),
            "xfin_hicc" => xf.entry((cc, true)).or_default().1 = Some(x),
            "xfout_locc" => xf.entry((cc, false)).or_default().0 = Some(x),
            "xfout_hicc" => xf.entry((cc, false)).or_default().1 = Some(x),
            s if s.ends_with("_curvecc") => {}
            s => {
                let base = (s.strip_suffix("_oncc"))
                    .or_else(|| s.strip_suffix("_cc"))
                    .or_else(|| s.strip_suffix("cc"))
                    .unwrap_or(s);
                let Some(t) = target(base) else {
                    continue;
                };
                let curve = int(ops.get(&format!("{base}_curvecc{cc}")))
                    .map_or(0, |c| c.clamp(0, 255) as u16);
                r.mods.push(Mod {
                    target: t,
                    cc,
                    amount: x,
                    curve,
                });
            }
        }
    }
    r.cc = ranges.into_iter().map(|(c, (a, b))| (c, a, b)).collect();
    r.on_cc = on.into_iter().next().map(|(c, (a, b))| (c, a, b));
    let ccp = power("xf_cccurve");
    for ((cc, rising), (lo, hi)) in xf {
        fade(Fader::Cc(cc), lo, hi, rising, ccp);
    }
    if r.on_cc.is_some() && ops.get("lokey").is_none() && ops.get("key").is_none() {
        // Started by its controller only.
        r.keys = None;
    }
    Some(r)
}

/// An opcode's name with the number on its end replaced by `N`, for
/// counting what is left out.
fn strip_number(k: &str) -> String {
    let n = k.bytes().rev().take_while(u8::is_ascii_digit).count();
    if n == 0 {
        k.to_string()
    } else {
        format!("{}N", &k[..k.len() - n])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(
        list: &'static [(&'static str, &'static str)],
    ) -> impl Fn(&Path) -> Result<String, String> {
        move |p: &Path| {
            let name = p.file_name().unwrap().to_string_lossy();
            (list.iter())
                .find(|(n, _)| *n == name)
                .map(|(_, t)| t.to_string())
                .ok_or_else(|| "not found".into())
        }
    }

    /// A path with Windows' separators as `/`, to compare on any system.
    fn slashed(p: &str) -> String {
        p.replace('\\', "/")
    }

    #[test]
    fn headers_pass_their_settings_down_and_includes_and_defines_are_followed() {
        let text = r#"
<control> default_path=samples/ label_cc70=Kick mic set_cc70=100
#define $KICK 36
#include "keys.sfz"
/* a block
   comment */
<global> loop_mode=one_shot seq_length=2 ampeg_release=0.2
<group> key=$KICK lovel=1 hivel=64 amplitude_cc70=100 locc70=1 group=1 off_by=1
<region> sample=kick soft 1.wav
<region> sample=kick soft 2.wav seq_position=2 // the other
<group> key=$KICK lovel=65 amp_velcurve_100=0.5
<region>sample=kick\hard.wav pan=-20 tune=-5
<region> sample=hat.wav key=$HAT trigger=release off_mode=normal
"#;
        let sfz = parse(
            text,
            Path::new("/lib"),
            &files(&[("keys.sfz", "#define $HAT f#2\n")]),
        );
        assert!(sfz.problems.is_empty(), "{:?}", sfz.problems);
        assert_eq!(sfz.regions.len(), 4);
        let r = &sfz.regions;
        assert_eq!(slashed(&r[0].sample), "/lib/samples/kick soft 1.wav");
        assert_eq!(r[0].keys, Some((36, 36)));
        assert_eq!(r[0].keycenter, 36);
        assert_eq!(r[0].vel, (1, 64));
        assert_eq!(r[0].loop_mode, LoopMode::OneShot);
        assert_eq!(r[0].eg.release, 0.2);
        assert_eq!((r[0].seq_length, r[0].seq_position), (2, 1));
        assert_eq!(r[1].seq_position, 2);
        assert_eq!(slashed(&r[1].sample), "/lib/samples/kick soft 2.wav");
        assert_eq!(r[0].cc, [(70, 1.0, 127.0)]);
        assert_eq!(
            r[0].mods,
            [Mod {
                target: Target::Amplitude,
                cc: 70,
                amount: 100.0,
                curve: 0
            }]
        );
        assert_eq!((r[0].group, r[0].off_by), (1, Some(1)));
        // The second group starts afresh.
        assert_eq!(r[2].vel, (65, 127));
        assert_eq!(slashed(&r[2].sample), "/lib/samples/kick/hard.wav");
        assert!(r[2].mods.is_empty());
        assert_eq!((r[2].pan, r[2].tune), (-20.0, -5.0));
        assert!((r[2].velocity_gain(100) - 0.5).abs() < 1e-6);
        assert!((r[2].velocity_gain(127) - 1.0).abs() < 1e-6);
        // F#2 is 42.
        assert_eq!(r[3].keys, Some((42, 42)));
        assert_eq!(r[3].trigger, Trigger::Release);
        assert_eq!(r[3].off_mode, OffMode::Normal);
        assert_eq!(sfz.cc_labels.get(&70).map(String::as_str), Some("Kick mic"));
        assert_eq!(sfz.cc_start.get(&70), Some(&100.0));
        assert_eq!(sfz.controllers(), [70]);
    }

    #[test]
    fn curves_velocity_and_notes() {
        let text = "<curve>curve_index=9\nv000=-1\nv063=0\nv127=1\n\
                    <region> sample=a.wav tune_oncc75=1200 tune_curvecc75=9 lokey=c4 hikey=b4";
        let sfz = parse(text, Path::new("/"), &files(&[]));
        assert_eq!(sfz.regions[0].keys, Some((60, 71)));
        let m = sfz.regions[0].mods[0];
        assert_eq!((m.target, m.curve), (Target::Tune, 9));
        assert!((sfz.curve(9, 63.0)).abs() < 1e-6);
        assert!((sfz.curve(9, 0.0) + 1.0).abs() < 1e-6);
        assert!((sfz.curve(9, 127.0) - 1.0).abs() < 1e-6);
        assert!((sfz.curve(0, 127.0) - 1.0).abs() < 1e-6);
        assert!((sfz.curve(1, 0.0) + 1.0).abs() < 1e-6);
        // The usual velocity curve falls as the square.
        let r = Region::default();
        assert!((r.velocity_gain(127) - 1.0).abs() < 1e-6);
        assert!((r.velocity_gain(64) - 0.254).abs() < 0.01);
        assert_eq!(note(Some(&"c#4".to_string())), Some(61));
        assert_eq!(note(Some(&"Bb3".to_string())), Some(58));
        assert_eq!(note(Some(&"c-1".to_string())), Some(0));
    }

    #[test]
    fn key_switches_and_what_is_left_out() {
        let text = "<group> sw_lokey=24 sw_hikey=26 sw_default=24\n\
                    <region> sample=a.wav sw_last=24 sw_label=Open\n\
                    <region> sample=b.wav sw_last=25 width=50 eq1_gain_oncc77=-3\n";
        let sfz = parse(text, Path::new("/"), &files(&[]));
        assert_eq!(sfz.sw_default, Some(24));
        assert_eq!(sfz.regions[0].sw_range, Some((24, 26)));
        let ks = sfz.key_switches();
        assert_eq!(ks[0], (24, "Open".to_string()));
        assert_eq!(ks[1].0, 25);
        assert_eq!(sfz.unsupported.get("width"), Some(&1));
        assert_eq!(sfz.unsupported.get("eq1_gain_onccN"), Some(&1));
    }

    #[test]
    fn big_rusty_drums_reads_when_it_is_here() {
        // The CC0 drum library this was tested with; skipped when absent.
        let Some(dir) = std::env::var_os("DAW_BIG_RUSTY").map(PathBuf::from) else {
            return;
        };
        let sfz = read(&dir.join("Programs/01-full.sfz")).unwrap();
        assert!(sfz.problems.is_empty(), "{:?}", sfz.problems);
        assert!(sfz.regions.len() > 3000, "{}", sfz.regions.len());
        let kicks = sfz.regions.iter().filter(|r| r.plays(36, 100)).count();
        assert!(kicks > 0);
        let missing: Vec<&str> = (sfz.regions.iter())
            .map(|r| r.sample.as_str())
            .filter(|s| !s.starts_with('*') && !Path::new(s).is_file())
            .take(5)
            .collect();
        assert!(missing.is_empty(), "{missing:?}");
        assert!(sfz.cc_labels.len() > 40);
        eprintln!("left out: {:?}", sfz.unsupported);
    }
}
