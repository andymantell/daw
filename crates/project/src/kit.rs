// SPDX-License-Identifier: GPL-3.0-or-later
//! Electronic drum kit profiles (roadmap 3.4): which note each pad and
//! zone of a kit sends, the hi-hat pedal's controller and range, and how
//! a cymbal is choked.
//!
//! A kit module keeps its pad notes in its own settings, so a profile
//! learned once describes that module on any computer. Profiles are
//! plain text files kept with the user's settings, not in a project;
//! the ones in `kits/` in this crate are built in, and a connected kit
//! is matched to one by its MIDI port name ([`for_port`]). With no
//! profile, the General MIDI drum map ([`KitProfile::general_midi`]) is
//! used.

/// A pad, or one zone of a pad, that a kit can send a note for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    /// Kept in profile files: never change one.
    pub id: &'static str,
    pub name: &'static str,
    /// What to do to make the kit send it, for the learn wizard.
    pub prompt: &'static str,
    /// Its note in the General MIDI drum map.
    pub gm: u8,
}

const fn slot(id: &'static str, name: &'static str, prompt: &'static str, gm: u8) -> Slot {
    Slot {
        id,
        name,
        prompt,
        gm,
    }
}

/// Every slot, in the order the learn wizard asks for them. The General
/// MIDI notes are from its percussion key map (notes 35 to 81); zones it
/// has no note of their own for (tom rims, cymbal edges) share the pad's.
pub const SLOTS: &[Slot] = &[
    slot("kick", "Kick", "Press the kick pedal.", 36),
    slot(
        "snare-head",
        "Snare head",
        "Hit the middle of the snare.",
        38,
    ),
    slot(
        "snare-rim",
        "Snare rim",
        "Hit the snare's rim (a rim shot).",
        40,
    ),
    slot(
        "snare-cross-stick",
        "Snare cross-stick",
        "Play a cross-stick on the snare, if your kit has one.",
        37,
    ),
    slot("tom1-head", "Tom 1 head", "Hit the highest tom.", 48),
    slot("tom1-rim", "Tom 1 rim", "Hit the highest tom's rim.", 48),
    slot("tom2-head", "Tom 2 head", "Hit the next tom down.", 45),
    slot("tom2-rim", "Tom 2 rim", "Hit that tom's rim.", 45),
    slot("tom3-head", "Tom 3 head", "Hit the next tom down.", 43),
    slot("tom3-rim", "Tom 3 rim", "Hit that tom's rim.", 43),
    slot(
        "tom4-head",
        "Tom 4 head",
        "Hit the lowest tom, if there is a fourth.",
        41,
    ),
    slot("tom4-rim", "Tom 4 rim", "Hit that tom's rim.", 41),
    slot(
        "hihat-open",
        "Hi-hat open",
        "With the hi-hat pedal up, hit the top of the hi-hat.",
        46,
    ),
    slot(
        "hihat-open-edge",
        "Hi-hat open edge",
        "With the pedal still up, hit the hi-hat's edge.",
        46,
    ),
    slot(
        "hihat-closed",
        "Hi-hat closed",
        "Hold the hi-hat pedal down and hit the top of the hi-hat.",
        42,
    ),
    slot(
        "hihat-closed-edge",
        "Hi-hat closed edge",
        "With the pedal still down, hit the hi-hat's edge.",
        42,
    ),
    slot(
        "hihat-pedal",
        "Hi-hat pedal",
        "Lift your foot and stamp the hi-hat pedal shut.",
        44,
    ),
    slot("crash1", "Crash 1", "Hit the top of the crash.", 49),
    slot("crash1-edge", "Crash 1 edge", "Hit the crash's edge.", 49),
    slot(
        "crash2",
        "Crash 2",
        "Hit the top of a second crash, if there is one.",
        57,
    ),
    slot("crash2-edge", "Crash 2 edge", "Hit that crash's edge.", 57),
    slot("ride", "Ride", "Hit the top of the ride.", 51),
    slot("ride-edge", "Ride edge", "Hit the ride's edge.", 51),
    slot("ride-bell", "Ride bell", "Hit the ride's bell.", 53),
];

/// The slot with this id.
pub fn slot_by_id(id: &str) -> Option<&'static Slot> {
    SLOTS.iter().find(|s| s.id == id)
}

/// Whether the notes in a profile are the ones the module came with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Factory {
    /// The module's own notes, as far as the owner knows: the profile
    /// suits every kit of that model.
    Factory,
    /// The owner changed some.
    Changed,
    #[default]
    Unknown,
}

impl Factory {
    pub const ALL: [Factory; 3] = [Factory::Factory, Factory::Changed, Factory::Unknown];

    fn id(self) -> &'static str {
        match self {
            Factory::Factory => "factory",
            Factory::Changed => "changed",
            Factory::Unknown => "unknown",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Factory::Factory => "The module's own notes",
            Factory::Changed => "Notes I changed",
            Factory::Unknown => "Not sure",
        }
    }
}

/// The hi-hat pedal's position, sent as a controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pedal {
    pub controller: u8,
    /// The value sent with the pedal all the way down.
    pub closed: u8,
    /// The value sent with it all the way up.
    pub open: u8,
}

impl Pedal {
    /// How open the hi-hat is for a value, from 0 (shut) to 1.
    pub fn openness(&self, value: u8) -> f32 {
        if self.open == self.closed {
            return 0.0;
        }
        let v = (value as f32 - self.closed as f32) / (self.open as f32 - self.closed as f32);
        v.clamp(0.0, 1.0)
    }
}

/// How the kit says a cymbal was grabbed to stop it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choke {
    /// Pressure on the cymbal's note (polyphonic aftertouch).
    KeyPressure,
    /// A controller.
    Control(u8),
}

/// One kit's map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KitProfile {
    pub name: String,
    /// MIDI port names it is recognised by; empty for none.
    pub ports: Vec<String>,
    /// Channel, 0 to 15; `None` for any.
    pub channel: Option<u8>,
    pub factory: Factory,
    /// Slot id and its note, in [`SLOTS`] order. A slot not listed is
    /// not on this kit.
    pub notes: Vec<(&'static str, u8)>,
    pub pedal: Option<Pedal>,
    pub choke: Option<Choke>,
}

/// The extension profile files have.
pub const KIT_EXTENSION: &str = "kit.txt";

/// The name the General MIDI map goes by.
pub const GENERAL_MIDI: &str = "General MIDI";

impl KitProfile {
    /// An empty profile: no pads yet.
    pub fn new(name: impl Into<String>) -> KitProfile {
        KitProfile {
            name: name.into(),
            ports: Vec::new(),
            channel: None,
            factory: Factory::Unknown,
            notes: Vec::new(),
            pedal: None,
            choke: None,
        }
    }

    /// The General MIDI drum map, on channel 10: what kits are commonly
    /// set to, and the map when nothing else is known.
    pub fn general_midi() -> KitProfile {
        KitProfile {
            channel: Some(9),
            factory: Factory::Factory,
            notes: SLOTS.iter().map(|s| (s.id, s.gm)).collect(),
            ..KitProfile::new(GENERAL_MIDI)
        }
    }

    /// The note for a slot, if the kit has it.
    pub fn note(&self, id: &str) -> Option<u8> {
        self.notes.iter().find(|(s, _)| *s == id).map(|(_, n)| *n)
    }

    /// Sets a slot's note, or takes the slot off with `None`.
    pub fn set(&mut self, id: &str, note: Option<u8>) {
        let Some(slot) = slot_by_id(id) else {
            return;
        };
        self.notes.retain(|(s, _)| *s != slot.id);
        if let Some(n) = note {
            self.notes.push((slot.id, n));
            self.notes
                .sort_by_key(|(s, _)| SLOTS.iter().position(|x| x.id == *s));
        }
    }

    /// The slots a note plays, first first. Two slots can share a note
    /// (a tom and its rim, say, when the module sends one for both).
    pub fn slots_for(&self, note: u8) -> impl Iterator<Item = &'static Slot> + '_ {
        (self.notes.iter())
            .filter(move |(_, n)| *n == note)
            .filter_map(|(s, _)| slot_by_id(s))
    }

    /// The General MIDI note a note from this kit stands for, so a part
    /// played on it sounds right on anything that follows the GM map.
    pub fn to_general_midi(&self, note: u8) -> Option<u8> {
        self.slots_for(note).next().map(|s| s.gm)
    }

    /// Whether the kit's MIDI input port is called `port`.
    pub fn matches_port(&self, port: &str) -> bool {
        let port = squash(port);
        (self.ports.iter())
            .map(|p| squash(p))
            .any(|p| !p.is_empty() && port.contains(&p))
    }

    /// The profile as a file.
    pub fn to_text(&self) -> String {
        let mut t = String::from("# Drum kit profile\n");
        t += &format!("name = {}\n", one_line(&self.name));
        for p in &self.ports {
            t += &format!("port = {}\n", one_line(p));
        }
        match self.channel {
            Some(c) => t += &format!("channel = {}\n", c + 1),
            None => t += "channel = any\n",
        }
        t += &format!("notes = {}\n", self.factory.id());
        for (id, n) in &self.notes {
            t += &format!("{id} = {n}\n");
        }
        if let Some(p) = self.pedal {
            t += &format!(
                "hihat-controller = {}, closed {}, open {}\n",
                p.controller, p.closed, p.open
            );
        }
        match self.choke {
            Some(Choke::KeyPressure) => t += "choke = pressure\n",
            Some(Choke::Control(c)) => t += &format!("choke = controller {c}\n"),
            None => {}
        }
        t
    }

    /// Reads a profile file. Lines it doesn't know are passed over, so
    /// files from a later version still read.
    pub fn from_text(text: &str) -> Result<KitProfile, String> {
        let mut k = KitProfile::new("");
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            match key {
                "name" => k.name = value.to_string(),
                "port" if !value.is_empty() => k.ports.push(value.to_string()),
                "channel" => {
                    k.channel = match value {
                        "any" => None,
                        v => match v.parse::<u8>() {
                            Ok(c @ 1..=16) => Some(c - 1),
                            _ => return Err(format!("The channel {v} isn't 1 to 16.")),
                        },
                    }
                }
                "notes" => {
                    k.factory = (Factory::ALL.into_iter())
                        .find(|f| f.id() == value)
                        .unwrap_or_default();
                }
                "hihat-controller" => k.pedal = Some(pedal(value)?),
                "choke" => {
                    k.choke = match value.split_whitespace().collect::<Vec<_>>()[..] {
                        ["pressure"] => Some(Choke::KeyPressure),
                        ["controller", n] => Some(Choke::Control(midi_value(n)?)),
                        _ => None,
                    }
                }
                id => {
                    if slot_by_id(id).is_some() {
                        k.set(id, Some(midi_value(value)?));
                    }
                }
            }
        }
        if k.name.is_empty() {
            return Err("The file has no kit name in it.".into());
        }
        Ok(k)
    }

    /// Pairs of slots that send the same note, where the module is
    /// likely to play one sound for both. Zones that share a note in
    /// General MIDI (a tom and its rim) don't count, nor do hi-hat zones,
    /// which some kits send as one note with the pedal saying how open.
    pub fn clashes(&self) -> Vec<(&'static Slot, &'static Slot)> {
        let mut out = Vec::new();
        let slots: Vec<(&Slot, u8)> = (self.notes.iter())
            .filter_map(|(s, n)| slot_by_id(s).map(|s| (s, *n)))
            .collect();
        for (i, (a, n)) in slots.iter().enumerate() {
            for (b, m) in &slots[i + 1..] {
                if n == m && a.gm != b.gm && !(hihat(a) && hihat(b)) {
                    out.push((*a, *b));
                }
            }
        }
        out
    }
}

fn hihat(s: &Slot) -> bool {
    s.id.starts_with("hihat")
}

/// The built-in profiles, from `kits/` in this crate. General MIDI is
/// always first.
pub fn built_in_kits() -> Vec<KitProfile> {
    let mut v = vec![KitProfile::general_midi()];
    v.extend(
        BUILT_IN
            .iter()
            .filter_map(|t| KitProfile::from_text(t).ok()),
    );
    v
}

/// Profile files built into the app. Add a learned profile's file to
/// `kits/` and list it here.
const BUILT_IN: &[&str] = &[];

/// The profile for a MIDI input port: the first in `profiles` that
/// knows the port's name.
pub fn for_port<'a>(profiles: &'a [KitProfile], port: &str) -> Option<&'a KitProfile> {
    profiles.iter().find(|k| k.matches_port(port))
}

/// Lower case letters and digits only, so names match across systems
/// that add their own words and numbers around a device's name.
fn squash(s: &str) -> String {
    (s.chars())
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn one_line(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

fn midi_value(s: &str) -> Result<u8, String> {
    match s.trim().parse::<u8>() {
        Ok(n) if n < 128 => Ok(n),
        _ => Err(format!("{s} isn't a MIDI value (0 to 127).")),
    }
}

/// `4, closed 127, open 0`.
fn pedal(s: &str) -> Result<Pedal, String> {
    let mut parts = s.split(',');
    let controller = midi_value(parts.next().unwrap_or(""))?;
    let (mut closed, mut open) = (127, 0);
    for p in parts {
        match p.split_whitespace().collect::<Vec<_>>()[..] {
            ["closed", v] => closed = midi_value(v)?,
            ["open", v] => open = midi_value(v)?,
            _ => {}
        }
    }
    Ok(Pedal {
        controller,
        closed,
        open,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_ids_are_unique() {
        for (i, s) in SLOTS.iter().enumerate() {
            assert!(SLOTS[i + 1..].iter().all(|t| t.id != s.id), "{}", s.id);
        }
    }

    #[test]
    fn general_midi_is_the_drum_map_on_channel_ten() {
        let gm = KitProfile::general_midi();
        assert_eq!(gm.channel, Some(9));
        assert_eq!(gm.note("kick"), Some(36));
        assert_eq!(gm.note("snare-head"), Some(38));
        assert_eq!(gm.note("hihat-closed"), Some(42));
        assert_eq!(gm.note("hihat-pedal"), Some(44));
        assert_eq!(gm.note("hihat-open"), Some(46));
        assert_eq!(gm.note("ride-bell"), Some(53));
        assert!(gm.clashes().is_empty());
        assert_eq!(built_in_kits()[0], gm);
    }

    #[test]
    fn a_profile_is_written_and_read_back() {
        let mut k = KitProfile::new("My kit");
        k.ports = vec!["EXS-5".into(), "USB MIDI".into()];
        k.channel = Some(9);
        k.factory = Factory::Changed;
        k.set("snare-head", Some(38));
        k.set("kick", Some(36));
        k.set("hihat-open", Some(8));
        k.pedal = Some(Pedal {
            controller: 4,
            closed: 127,
            open: 0,
        });
        k.choke = Some(Choke::Control(11));
        // Kept in slot order, whatever order they were learned in.
        assert_eq!(k.notes[0], ("kick", 36));
        let text = k.to_text();
        assert!(text.contains("kick = 36\n"));
        assert!(text.contains("channel = 10\n"));
        assert_eq!(KitProfile::from_text(&text).unwrap(), k);
        k.choke = Some(Choke::KeyPressure);
        k.channel = None;
        assert_eq!(KitProfile::from_text(&k.to_text()).unwrap(), k);
    }

    #[test]
    fn bad_files_say_why_and_unknown_lines_are_passed_over() {
        assert!(KitProfile::from_text("kick = 36").is_err());
        assert!(KitProfile::from_text("name = A\nkick = 300").is_err());
        assert!(KitProfile::from_text("name = A\nchannel = 17").is_err());
        let k = KitProfile::from_text("name = A\ncowbell-zone = 3\nlater = x\nkick = 35").unwrap();
        assert_eq!(k.notes, [("kick", 35)]);
    }

    #[test]
    fn a_port_is_recognised_whatever_the_system_adds_to_its_name() {
        let mut k = KitProfile::new("Kit");
        k.ports = vec!["EXS-5".into()];
        assert!(k.matches_port("EXS-5"));
        assert!(k.matches_port("EXS5 MIDI 1 20:0"));
        assert!(k.matches_port("2- exs-5"));
        assert!(!k.matches_port("Keyboard"));
        assert!(!KitProfile::new("None").matches_port("EXS-5"));
        let list = [KitProfile::general_midi(), k];
        assert_eq!(
            for_port(&list, "EXS-5 0").map(|k| k.name.as_str()),
            Some("Kit")
        );
        assert!(for_port(&list, "Piano").is_none());
    }

    #[test]
    fn notes_map_to_their_pads_and_to_general_midi() {
        let mut k = KitProfile::new("Kit");
        k.set("snare-head", Some(10));
        k.set("snare-rim", Some(11));
        assert_eq!(k.slots_for(11).next().map(|s| s.id), Some("snare-rim"));
        assert_eq!(k.to_general_midi(10), Some(38));
        assert_eq!(k.to_general_midi(99), None);
        k.set("snare-rim", None);
        assert_eq!(k.note("snare-rim"), None);
    }

    #[test]
    fn two_pads_on_one_note_are_a_clash_but_a_tom_and_its_rim_are_not() {
        let mut k = KitProfile::new("Kit");
        k.set("tom1-head", Some(48));
        k.set("tom1-rim", Some(48));
        assert!(k.clashes().is_empty());
        k.set("hihat-open", Some(8));
        k.set("hihat-closed", Some(8));
        assert!(k.clashes().is_empty());
        k.set("kick", Some(48));
        assert_eq!(k.clashes().len(), 2);
    }

    #[test]
    fn the_pedal_says_how_open_the_hi_hat_is() {
        let p = Pedal {
            controller: 4,
            closed: 127,
            open: 0,
        };
        assert_eq!(p.openness(127), 0.0);
        assert_eq!(p.openness(0), 1.0);
        assert!((p.openness(64) - 0.496).abs() < 0.01);
    }
}
