// SPDX-License-Identifier: GPL-3.0-or-later
//! Review mode (Phase 4, B1): suggested edits made on a branch of the
//! project, which the people the song belongs to accept or reject one
//! change at a time. A teacher can suggest edits to a student's take
//! without overwriting it.
//!
//! A suggestion lives in the document's `suggestions` map, by id: its
//! `name`, `author` (the member's key, as hex), `created`, `base` (the
//! document's frontiers when it was started, encoded), `edits` (the
//! changes made on the branch since then, as Loro updates) and
//! `decisions` (a map from a change's key to "accepted" or
//! "rejected"). The branch is a fork of the document at `base` with
//! `edits` imported, so it shares the song's history and is only ever
//! read, never merged into the song: the song changes only when a change
//! is accepted, as one ordinary, undoable edit.
//!
//! A change is one thing that can be taken on its own: a song setting
//! (tempo, key, loop…), one marker, tempo change, pattern or other item
//! by id, a whole track added or removed, or, for a track on both sides,
//! its settings (everything but its clips), one clip or one note clip.
//! What a change says comes from the snapshot diff ([`differences`])
//! between the base and the base with only that change taken.
//!
//! Comments, assignments and each person's practice aren't suggested:
//! they are left as they are.

use std::collections::HashSet;

use loro::{ExportMode, Frontiers, LoroDoc, LoroMap, LoroValue};

use crate::doc::QUIET;
use crate::{Project, ProjectDoc, Track, differences, new_id};

/// Where suggestions live in the document.
pub(crate) const SUGGESTIONS: &str = "suggestions";

/// A suggestion's name is cut to this many characters.
pub const MAX_SUGGESTION_NAME: usize = 80;

/// A set of suggested edits.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub id: String,
    pub name: String,
    /// Who made it: the member's public key, as hex, or empty.
    pub author: String,
    /// When it was started, in milliseconds since 1970.
    pub created: i64,
    /// Whether this copy has the version it started from, so its
    /// changes can be read.
    pub readable: bool,
}

/// What was decided about a suggested change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Accepted,
    Rejected,
}

impl Decision {
    fn word(self) -> &'static str {
        match self {
            Decision::Accepted => "accepted",
            Decision::Rejected => "rejected",
        }
    }

    fn from_word(w: &str) -> Option<Decision> {
        match w {
            "accepted" => Some(Decision::Accepted),
            "rejected" => Some(Decision::Rejected),
            _ => None,
        }
    }
}

/// One suggested change, as the review shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct SuggestedChange {
    /// What names it, to accept or reject it: "tempo",
    /// "marker/<id>", "track/<id>", "track/<id>/settings",
    /// "track/<id>/clip/<id>", "track/<id>/note_clip/<id>"…
    pub key: String,
    /// Where: "Song", a track's name, "Markers"…
    pub place: String,
    /// What it does, in words, a line each.
    pub text: Vec<String>,
    pub decision: Option<Decision>,
    /// The song already has it, by someone's edit since or by accepting
    /// it.
    pub in_song: bool,
    /// The same thing has been changed in the song since the suggestion
    /// started, so accepting it replaces that change.
    pub changed_since: bool,
}

impl SuggestedChange {
    /// Still waiting for a decision.
    pub fn open(&self) -> bool {
        self.decision.is_none() && !self.in_song
    }
}

impl ProjectDoc {
    /// Starts a suggestion from the project as it is now, and returns
    /// its id. Make the edits on [`suggestion_branch`]
    /// (Self::suggestion_branch) and keep them with
    /// [`keep_suggestion`](Self::keep_suggestion). Not undoable.
    pub fn start_suggestion(&mut self, name: &str, author: &str, created: i64) -> String {
        self.doc.commit();
        let base = self.doc.oplog_frontiers().encode();
        let id = new_id();
        let all = self.doc.get_map(SUGGESTIONS);
        if let Ok(m) = all.insert_container(&id, LoroMap::new()) {
            let _ = m.insert("name", clean(name).as_str());
            let _ = m.insert("author", author);
            let _ = m.insert("created", created);
            let _ = m.insert("base", base);
            let _ = m.insert("edits", Vec::<u8>::new());
            let _ = m.insert_container("decisions", LoroMap::new());
        }
        self.commit_suggestions();
        id
    }

    /// Every suggestion, newest first.
    pub fn suggestions(&self) -> Vec<Suggestion> {
        let LoroValue::Map(all) = self.doc.get_map(SUGGESTIONS).get_deep_value() else {
            return Vec::new();
        };
        let mut v: Vec<Suggestion> = (all.iter())
            .filter_map(|(id, m)| {
                let LoroValue::Map(m) = m else { return None };
                let text = |k: &str| match m.get(k) {
                    Some(LoroValue::String(s)) => s.to_string(),
                    _ => String::new(),
                };
                Some(Suggestion {
                    id: id.to_string(),
                    name: text("name"),
                    author: text("author"),
                    created: match m.get("created") {
                        Some(LoroValue::I64(t)) => *t,
                        _ => 0,
                    },
                    readable: self.suggestion_base(id).is_some(),
                })
            })
            .collect();
        v.sort_by(|a, b| b.created.cmp(&a.created).then(b.id.cmp(&a.id)));
        v
    }

    /// The branch a suggestion is made on, with its edits so far, to
    /// make more. `None` if there is no such suggestion or this copy
    /// can't read it.
    pub fn suggestion_branch(&self, id: &str) -> Option<ProjectDoc> {
        let base = self.suggestion_base(id)?;
        let doc = self.doc.fork_at(&base).ok()?;
        let edits = self.suggestion_edits(id);
        if !edits.is_empty() {
            doc.import(&edits).ok()?;
        }
        Some(ProjectDoc::wrap(doc))
    }

    /// Keeps the edits made on `branch` (from
    /// [`suggestion_branch`](Self::suggestion_branch)) as the
    /// suggestion's. Not undoable. False if there is no such suggestion.
    pub fn keep_suggestion(&mut self, id: &str, branch: &ProjectDoc) -> bool {
        let (Some(base), Some(m)) = (self.suggestion_base(id), self.suggestion_map(id)) else {
            return false;
        };
        branch.doc.commit();
        let Ok(vv) = branch.doc.frontiers_to_vv(&base).ok_or(()) else {
            return false;
        };
        let Ok(edits) = branch.doc.export(ExportMode::updates(&vv)) else {
            return false;
        };
        if edits == self.suggestion_edits(id) {
            return true;
        }
        let _ = m.insert("edits", edits);
        self.commit_suggestions();
        true
    }

    /// Gives a suggestion a new name. False if there is no such
    /// suggestion.
    pub fn rename_suggestion(&mut self, id: &str, name: &str) -> bool {
        let Some(m) = self.suggestion_map(id) else {
            return false;
        };
        let _ = m.insert("name", clean(name).as_str());
        self.commit_suggestions();
        true
    }

    /// Forgets a suggestion, decided or not. The song is untouched.
    /// False if there was no such suggestion.
    pub fn delete_suggestion(&mut self, id: &str) -> bool {
        let all = self.doc.get_map(SUGGESTIONS);
        if all.get(id).is_none() {
            return false;
        }
        let _ = all.delete(id);
        self.commit_suggestions();
        true
    }

    /// The project as the suggestion would have it, comments and
    /// assignments aside (those are the ones there are now).
    pub fn suggested_project(&self, id: &str) -> Option<Project> {
        let mut p = self.suggestion_branch(id)?.get();
        let now = self.get();
        p.comments = now.comments;
        p.assignments = now.assignments;
        Some(p)
    }

    /// The project as it was when the suggestion started.
    pub fn suggestion_base_project(&self, id: &str) -> Option<Project> {
        let base = self.suggestion_base(id)?;
        Some(ProjectDoc::wrap(self.doc.fork_at(&base).ok()?).get())
    }

    /// Each change the suggestion makes, song first, then each track,
    /// then markers and the rest. `None` if it can't be read.
    pub fn suggested_changes(&self, id: &str) -> Option<Vec<SuggestedChange>> {
        let base = self.suggestion_base_project(id)?;
        let wanted = self.suggested_project(id)?;
        let now = self.get();
        let decided = self.decisions(id);
        let out = change_keys(&base, &wanted)
            .into_iter()
            .map(|key| {
                let mut taken = base.clone();
                let _ = take(&key, &wanted, &mut taken);
                let said = differences(&base, &taken);
                let place = said
                    .first()
                    .map_or_else(|| place_of(&key, &wanted, &base), |d| d.place.clone());
                let text = match said.is_empty() {
                    true => vec![format!("Changed {}", what(&key))],
                    false => said.into_iter().map(|d| d.text).collect(),
                };
                SuggestedChange {
                    decision: decided.iter().find(|(k, _)| *k == key).map(|(_, d)| *d),
                    in_song: same(&key, &now, &wanted),
                    changed_since: !same(&key, &now, &base),
                    key,
                    place,
                    text,
                }
            })
            .collect();
        Some(out)
    }

    /// Takes one suggested change into `into` (the song as it is now,
    /// or about to be written), without writing anything, and returns
    /// the undo step's name for it. Errors say why it can't be: no such
    /// suggestion or change, or the track it's on has gone.
    pub fn take_change(&self, id: &str, key: &str, into: &mut Project) -> Result<String, String> {
        let wanted = self
            .suggested_project(id)
            .ok_or("There's no such suggestion, or it can't be read here")?;
        let base = self
            .suggestion_base_project(id)
            .ok_or("This suggestion can't be read here")?;
        if !change_keys(&base, &wanted).iter().any(|k| k == key) {
            return Err(format!("The suggestion has no change “{key}”"));
        }
        take(key, &wanted, into)?;
        let mut taken = base.clone();
        let _ = take(key, &wanted, &mut taken);
        Ok(match differences(&base, &taken).first() {
            Some(d) => format!("Accept suggestion: {}", d.text),
            None => "Accept suggestion".to_string(),
        })
    }

    /// Takes one suggested change into the song, as one undoable edit,
    /// marks it accepted, and returns the song as it is then.
    pub fn accept_change(&mut self, id: &str, key: &str) -> Result<Project, String> {
        let mut p = self.get();
        let label = self.take_change(id, key, &mut p)?;
        self.set_labelled(&p, &label);
        self.mark_change(id, key, Decision::Accepted);
        Ok(self.get())
    }

    /// Records a decision about a change, without touching the song.
    /// False if there's no such suggestion.
    pub fn mark_change(&mut self, id: &str, key: &str, d: Decision) -> bool {
        if self.suggestion_map(id).is_none() {
            return false;
        }
        self.decide(id, key, d);
        true
    }

    /// Turns a suggested change down: the song is left as it is. False
    /// if there's no such suggestion.
    pub fn reject_change(&mut self, id: &str, key: &str) -> bool {
        if self.suggestion_map(id).is_none() {
            return false;
        }
        self.decide(id, key, Decision::Rejected);
        true
    }

    /// What has been decided about a suggestion's changes, by key.
    fn decisions(&self, id: &str) -> Vec<(String, Decision)> {
        let Some(m) = self.suggestion_map(id) else {
            return Vec::new();
        };
        let Some(Ok(d)) = m.get("decisions").map(|v| v.into_container()) else {
            return Vec::new();
        };
        let Ok(d) = d.into_map() else {
            return Vec::new();
        };
        let LoroValue::Map(all) = d.get_deep_value() else {
            return Vec::new();
        };
        (all.iter())
            .filter_map(|(k, v)| match v {
                LoroValue::String(s) => Some((k.to_string(), Decision::from_word(s)?)),
                _ => None,
            })
            .collect()
    }

    fn decide(&mut self, id: &str, key: &str, d: Decision) {
        let Some(m) = self.suggestion_map(id) else {
            return;
        };
        let existing = (m.get("decisions"))
            .and_then(|v| v.into_container().ok())
            .and_then(|c| c.into_map().ok());
        let Some(decisions) =
            existing.or_else(|| m.insert_container("decisions", LoroMap::new()).ok())
        else {
            return;
        };
        let _ = decisions.insert(key, d.word());
        self.commit_suggestions();
    }

    fn suggestion_map(&self, id: &str) -> Option<LoroMap> {
        self.doc
            .get_map(SUGGESTIONS)
            .get(id)?
            .into_container()
            .ok()?
            .into_map()
            .ok()
    }

    /// The version a suggestion started from, if this copy has it.
    fn suggestion_base(&self, id: &str) -> Option<Frontiers> {
        let m = self.suggestion_map(id)?;
        let LoroValue::Binary(b) = m.get("base")?.into_value().ok()? else {
            return None;
        };
        let f = Frontiers::decode(&b).ok()?;
        let vv = self.doc.oplog_vv();
        let all_here = f.iter().all(|id| vv.includes_id(id));
        all_here.then_some(f)
    }

    fn suggestion_edits(&self, id: &str) -> Vec<u8> {
        let Some(m) = self.suggestion_map(id) else {
            return Vec::new();
        };
        match m.get("edits").and_then(|v| v.into_value().ok()) {
            Some(LoroValue::Binary(b)) => b.to_vec(),
            _ => Vec::new(),
        }
    }

    fn commit_suggestions(&mut self) {
        self.doc.set_next_commit_origin(QUIET);
        self.doc.commit();
    }
}

/// Who made each suggestion, by id, for the identity check: a
/// commenter may make and change only their own.
pub(crate) fn suggestion_authors(doc: &LoroDoc) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    if let LoroValue::Map(all) = doc.get_map(SUGGESTIONS).get_deep_value() {
        for (id, s) in all.iter() {
            if let LoroValue::Map(s) = s
                && let Some(LoroValue::String(a)) = s.get("author")
            {
                out.insert(id.to_string(), a.to_string());
            }
        }
    }
    out
}

/// A name with no line breaks or surrounding space, not too long.
fn clean(name: &str) -> String {
    let one_line: String = name
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    one_line.trim().chars().take(MAX_SUGGESTION_NAME).collect()
}

// The changes themselves.

/// Something kept in a list by id.
trait Keyed: Clone + PartialEq {
    fn key(&self) -> &str;
}

macro_rules! keyed {
    ($($t:ty),*) => {
        $(impl Keyed for $t {
            fn key(&self) -> &str {
                &self.id
            }
        })*
    };
}

keyed!(
    crate::TempoChange,
    crate::MeterChange,
    crate::Marker,
    crate::TabBlock,
    crate::EditGroup,
    crate::ControlGroup,
    crate::CueMix,
    crate::HitPoint,
    crate::Groove,
    crate::MidiMap,
    crate::Pattern,
    crate::VideoClip,
    crate::Clip,
    crate::NoteClip,
    Track
);

/// The song's settings that are each one change.
const SETTINGS: [&str; 9] = [
    "tempo",
    "meter",
    "key",
    "loop",
    "reference",
    "timecode",
    "arrangement",
    "master",
    "notes",
];

/// The lists whose items are each one change, by the key's first part.
const LISTS: [&str; 12] = [
    "tempo_change",
    "meter_change",
    "marker",
    "tab",
    "edit_group",
    "control_group",
    "cue_mix",
    "hit_point",
    "groove",
    "midi_map",
    "pattern",
    "video",
];

/// The ids of a list's items, by the list's name in [`LISTS`].
fn list_ids(p: &Project, list: &str) -> Vec<String> {
    fn ids<T: Keyed>(v: &[T]) -> Vec<String> {
        v.iter().map(|x| x.key().to_string()).collect()
    }
    match list {
        "tempo_change" => ids(&p.tempo_changes),
        "meter_change" => ids(&p.meter_changes),
        "marker" => ids(&p.markers),
        "tab" => ids(&p.tabs),
        "edit_group" => ids(&p.edit_groups),
        "control_group" => ids(&p.control_groups),
        "cue_mix" => ids(&p.cue_mixes),
        "hit_point" => ids(&p.hit_points),
        "groove" => ids(&p.grooves),
        "midi_map" => ids(&p.midi_maps),
        "pattern" => ids(&p.patterns),
        "video" => ids(&p.videos),
        _ => Vec::new(),
    }
}

/// Every change from `base` to `wanted`, by key, in the order shown.
fn change_keys(base: &Project, wanted: &Project) -> Vec<String> {
    let mut keys: Vec<String> = SETTINGS.iter().map(|s| s.to_string()).collect();
    let track_ids = union(
        base.tracks.iter().map(|t| t.id.clone()).collect(),
        wanted.tracks.iter().map(|t| t.id.clone()).collect(),
    );
    for id in track_ids {
        let (b, w) = (track(base, &id), track(wanted, &id));
        match (b, w) {
            (Some(b), Some(w)) => {
                keys.push(format!("track/{id}/settings"));
                let clips = union(ids_of(&b.clips), ids_of(&w.clips));
                keys.extend(clips.iter().map(|c| format!("track/{id}/clip/{c}")));
                let notes = union(ids_of(&b.note_clips), ids_of(&w.note_clips));
                keys.extend(notes.iter().map(|c| format!("track/{id}/note_clip/{c}")));
            }
            _ => keys.push(format!("track/{id}")),
        }
    }
    for list in LISTS {
        let ids = union(list_ids(base, list), list_ids(wanted, list));
        keys.extend(ids.iter().map(|i| format!("{list}/{i}")));
    }
    keys.retain(|k| !same(k, base, wanted));
    keys
}

fn ids_of<T: Keyed>(v: &[T]) -> Vec<String> {
    v.iter().map(|x| x.key().to_string()).collect()
}

/// `b` after `a`, without the ids already in `a`, keeping their order.
fn union(a: Vec<String>, b: Vec<String>) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for id in b.into_iter().chain(a) {
        if seen.insert(id.clone()) {
            out.push(id);
        }
    }
    out
}

fn track<'a>(p: &'a Project, id: &str) -> Option<&'a Track> {
    p.tracks.iter().find(|t| t.id == id)
}

/// Whether `a` and `b` agree on the change named by `key`: taking it
/// from `b` would leave `a` as it is.
fn same(key: &str, a: &Project, b: &Project) -> bool {
    let mut x = a.clone();
    take(key, b, &mut x).is_ok() && x == *a
}

/// Makes `into` have what `from` has for the change named by `key`.
fn take(key: &str, from: &Project, into: &mut Project) -> Result<(), String> {
    let parts: Vec<&str> = key.splitn(4, '/').collect();
    match parts.as_slice() {
        ["tempo"] => into.tempo_bpm = from.tempo_bpm,
        ["meter"] => {
            into.beats_per_bar = from.beats_per_bar;
            into.beat_unit = from.beat_unit;
        }
        ["key"] => into.scale = from.scale,
        ["loop"] => into.loop_bars = from.loop_bars,
        ["reference"] => into.reference_hz = from.reference_hz,
        ["timecode"] => into.timecode = from.timecode.clone(),
        ["arrangement"] => into.arrangement = from.arrangement.clone(),
        ["master"] => into.master = from.master.clone(),
        ["notes"] => into.notes = from.notes.clone(),
        ["tempo_change", id] => put(&from.tempo_changes, &mut into.tempo_changes, id),
        ["meter_change", id] => put(&from.meter_changes, &mut into.meter_changes, id),
        ["marker", id] => put(&from.markers, &mut into.markers, id),
        ["tab", id] => put(&from.tabs, &mut into.tabs, id),
        ["edit_group", id] => put(&from.edit_groups, &mut into.edit_groups, id),
        ["control_group", id] => put(&from.control_groups, &mut into.control_groups, id),
        ["cue_mix", id] => put(&from.cue_mixes, &mut into.cue_mixes, id),
        ["hit_point", id] => put(&from.hit_points, &mut into.hit_points, id),
        ["groove", id] => put(&from.grooves, &mut into.grooves, id),
        ["midi_map", id] => put(&from.midi_maps, &mut into.midi_maps, id),
        ["pattern", id] => put(&from.patterns, &mut into.patterns, id),
        ["video", id] => put(&from.videos, &mut into.videos, id),
        ["track", id] => put(&from.tracks, &mut into.tracks, id),
        ["track", id, rest @ ..] => {
            let gone = || "The track it's on has been deleted since".to_string();
            let src = track(from, id).ok_or_else(gone)?;
            let dst = (into.tracks.iter_mut().find(|t| t.id == *id)).ok_or_else(gone)?;
            match rest {
                ["settings"] => {
                    let clips = std::mem::take(&mut dst.clips);
                    let notes = std::mem::take(&mut dst.note_clips);
                    *dst = src.clone();
                    dst.clips = clips;
                    dst.note_clips = notes;
                }
                ["clip", c] => put(&src.clips, &mut dst.clips, c),
                ["note_clip", c] => put(&src.note_clips, &mut dst.note_clips, c),
                _ => return Err(format!("No change is called “{key}”")),
            }
        }
        _ => return Err(format!("No change is called “{key}”")),
    }
    // Lists kept in order of time.
    into.markers.sort_by_key(|m| m.bar);
    Ok(())
}

/// Makes `into` have `from`'s item `id`, or not have it if `from`
/// doesn't. A new item goes after the nearest one before it in `from`
/// that `into` has.
fn put<T: Keyed>(from: &[T], into: &mut Vec<T>, id: &str) {
    let Some(i) = from.iter().position(|x| x.key() == id) else {
        into.retain(|x| x.key() != id);
        return;
    };
    if let Some(j) = into.iter().position(|x| x.key() == id) {
        into[j] = from[i].clone();
        return;
    }
    let at = (from[..i].iter().rev())
        .find_map(|p| into.iter().position(|x| x.key() == p.key()))
        .map_or(0, |j| j + 1);
    into.insert(at, from[i].clone());
}

/// Where a change is, when the diff says nothing about it.
fn place_of(key: &str, wanted: &Project, base: &Project) -> String {
    match key.split('/').collect::<Vec<_>>().as_slice() {
        ["track", id, ..] => {
            (track(wanted, id).or(track(base, id))).map_or("Tracks".into(), |t| t.name.clone())
        }
        [s] if SETTINGS.contains(s) => "Song".into(),
        [list, ..] => list.replace('_', " "),
        _ => String::new(),
    }
}

/// A change's kind in words, when the diff says nothing about it.
fn what(key: &str) -> String {
    match key.split('/').collect::<Vec<_>>().as_slice() {
        ["track", _] => "a track".into(),
        ["track", _, "settings"] => "the track's settings".into(),
        ["track", _, "clip", _] => "a clip".into(),
        ["track", _, "note_clip", _] => "a note clip".into(),
        [s] => format!("the {s}"),
        [list, ..] => format!("a {}", list.replace('_', " ")),
        _ => key.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Clip, Marker, Track};

    fn clip(id: &str, start: i64) -> Clip {
        Clip {
            id: id.into(),
            asset: "a.wav".into(),
            name: "take".into(),
            start,
            len: 48_000,
            ..Default::default()
        }
    }

    fn student() -> ProjectDoc {
        let mut doc = ProjectDoc::new(1);
        let mut p = Project::default();
        let mut t = Track::new("gtr".into(), "Guitar".into());
        t.clips = vec![clip("c1", 0), clip("c2", 96_000)];
        p.tracks.push(t);
        p.markers.push(Marker {
            id: "m1".into(),
            name: "Verse".into(),
            bar: 1,
        });
        doc.set(&p);
        doc
    }

    /// The teacher's suggestion: tempo 120 → 100, the second clip moved
    /// a bar later, a chorus marker, and the guitar 3 dB quieter.
    fn suggest(doc: &mut ProjectDoc) -> String {
        let id = doc.start_suggestion("Lesson 4 ideas", "teacher", 10);
        let mut branch = doc.suggestion_branch(&id).unwrap();
        let mut p = branch.get();
        p.tempo_bpm = 100.0;
        p.tracks[0].clips[1].start = 144_000;
        p.tracks[0].gain_db = -3.0;
        p.markers.push(Marker {
            id: "m2".into(),
            name: "Chorus".into(),
            bar: 9,
        });
        branch.set(&p);
        assert!(doc.keep_suggestion(&id, &branch));
        id
    }

    #[test]
    fn a_suggestion_leaves_the_song_alone() {
        let mut doc = student();
        let before = doc.get();
        let id = suggest(&mut doc);
        assert_eq!(doc.get(), before);
        assert_eq!(doc.suggested_project(&id).unwrap().tempo_bpm, 100.0);
        let list = doc.suggestions();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Lesson 4 ideas");
        assert!(list[0].readable);
    }

    #[test]
    fn each_change_is_listed_on_its_own() {
        let mut doc = student();
        let id = suggest(&mut doc);
        let changes = doc.suggested_changes(&id).unwrap();
        let keys: Vec<&str> = changes.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "tempo",
                "track/gtr/settings",
                "track/gtr/clip/c2",
                "marker/m2"
            ]
        );
        assert_eq!(changes[0].text, ["Tempo 120 → 100 BPM"]);
        assert_eq!(changes[1].place, "Guitar");
        assert!(changes.iter().all(SuggestedChange::open));
    }

    #[test]
    fn accepting_takes_only_that_change() {
        let mut doc = student();
        let id = suggest(&mut doc);
        let p = doc.accept_change(&id, "track/gtr/clip/c2").unwrap();
        assert_eq!(p.tracks[0].clips[1].start, 144_000);
        assert_eq!(p.tempo_bpm, 120.0, "the tempo waits for its own decision");
        assert_eq!(p.tracks[0].gain_db, 0.0);
        assert!(doc.reject_change(&id, "tempo"));
        let changes = doc.suggested_changes(&id).unwrap();
        let c2 = changes
            .iter()
            .find(|c| c.key == "track/gtr/clip/c2")
            .unwrap();
        assert_eq!(c2.decision, Some(Decision::Accepted));
        assert!(c2.in_song);
        let tempo = changes.iter().find(|c| c.key == "tempo").unwrap();
        assert_eq!(tempo.decision, Some(Decision::Rejected));
        assert_eq!(changes.iter().filter(|c| c.open()).count(), 2);
        // Accepting is one undoable edit.
        assert!(doc.undo());
        assert_eq!(doc.get().tracks[0].clips[1].start, 96_000);
    }

    #[test]
    fn a_new_marker_goes_in_and_a_removed_one_goes() {
        let mut doc = student();
        let id = doc.start_suggestion("Tidy", "t", 1);
        let mut branch = doc.suggestion_branch(&id).unwrap();
        let mut p = branch.get();
        p.markers.clear();
        branch.set(&p);
        doc.keep_suggestion(&id, &branch);
        let p = doc.accept_change(&id, "marker/m1").unwrap();
        assert!(p.markers.is_empty());
    }

    #[test]
    fn edits_since_are_flagged_and_kept_until_accepted() {
        let mut doc = student();
        let id = suggest(&mut doc);
        // The student changes the tempo themselves meanwhile.
        let mut p = doc.get();
        p.tempo_bpm = 110.0;
        doc.set(&p);
        let changes = doc.suggested_changes(&id).unwrap();
        let tempo = changes.iter().find(|c| c.key == "tempo").unwrap();
        assert!(tempo.changed_since);
        let clip = changes
            .iter()
            .find(|c| c.key == "track/gtr/clip/c2")
            .unwrap();
        assert!(!clip.changed_since);
        // The suggestion's own view of the song doesn't take the
        // student's later edit.
        assert_eq!(doc.suggestion_base_project(&id).unwrap().tempo_bpm, 120.0);
        assert_eq!(doc.accept_change(&id, "tempo").unwrap().tempo_bpm, 100.0);
    }

    #[test]
    fn a_track_deleted_since_says_so() {
        let mut doc = student();
        let id = suggest(&mut doc);
        let mut p = doc.get();
        p.tracks.clear();
        doc.set(&p);
        let e = doc.accept_change(&id, "track/gtr/clip/c2").unwrap_err();
        assert!(e.contains("deleted"), "{e}");
        assert!(doc.accept_change(&id, "no/such").is_err());
    }

    #[test]
    fn a_suggestion_can_be_carried_on_and_travels_with_the_project() {
        let mut doc = student();
        let id = suggest(&mut doc);
        let mut branch = doc.suggestion_branch(&id).unwrap();
        let mut p = branch.get();
        assert_eq!(p.tempo_bpm, 100.0, "the edits so far come back");
        p.loop_bars = (9, 17);
        branch.set(&p);
        doc.keep_suggestion(&id, &branch);
        let copy = ProjectDoc::from_bytes(&doc.to_bytes(), 2).unwrap();
        let keys: Vec<String> = (copy.suggested_changes(&id).unwrap())
            .into_iter()
            .map(|c| c.key)
            .collect();
        assert!(keys.contains(&"loop".to_string()), "{keys:?}");
        assert!(copy.delete_suggestion_works(&id));
    }

    impl ProjectDoc {
        fn delete_suggestion_works(mut self, id: &str) -> bool {
            self.delete_suggestion(id) && self.suggestions().is_empty()
        }
    }
}
