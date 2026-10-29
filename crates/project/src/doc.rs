// SPDX-License-Identifier: GPL-3.0-or-later
//! The project document on Loro.
//!
//! Layout: a `song` map of settings, a `tracks` map from id to track,
//! with the track order in the movable list `order`, a `markers` map, and
//! the lesson notes as text, and a `comments` map from id to comment. A
//! track is a map of its settings, a `clips`
//! map from id to clip, an `fx` map from id to effect and the effect
//! order in the movable list `fx_order`. Every field is its own map entry,
//! so two people changing different fields of the same clip both keep
//! their change. A `practice` map holds each person's practice settings
//! under their own key, which only they write.
//!
//! Deleting a comment, or changing its text, has to take the old words
//! out of the file too, not just out of the project as it stands: the
//! document keeps its whole history, so they could otherwise be read
//! back. [`ProjectDoc::to_bytes`] blanks every value that comments no
//! longer show in the copy it writes (see [`scrub`]).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use loro::{
    ContainerID, ContainerTrait, ExportMode, JsonMapOp, JsonOpContent, JsonSchema, LoroDoc,
    LoroError, LoroMap, LoroMapValue, LoroMovableList, LoroText, LoroValue, UndoManager,
    UpdateOptions, VersionVector,
};

use crate::{
    Clip, Comment, Effect, EffectState, Marker, Practice, Project, Track, TrainerSettings,
};

/// Commits with this origin (plugin state written at save time) are left
/// out of undo.
const QUIET: &str = "quiet";

pub struct ProjectDoc {
    doc: LoroDoc,
    undo: UndoManager,
    /// Whether the history may hold comment text that comments no longer
    /// show, so saving has to scrub it.
    scrub: bool,
    /// Whether a comment was deleted or changed since
    /// [`take_dropped`](Self::take_dropped) was last asked.
    dropped: bool,
    /// The last scrubbed copy, and the version it was made at.
    scrubbed: RefCell<Option<(VersionVector, Vec<u8>)>>,
}

impl ProjectDoc {
    /// An empty project, edited as `peer`.
    pub fn new(peer: u64) -> ProjectDoc {
        let doc = LoroDoc::new();
        // A fresh document accepts any peer id.
        let _ = doc.set_peer_id(peer);
        ProjectDoc::wrap(doc)
    }

    /// A saved project, edited from here on as `peer`.
    pub fn from_bytes(bytes: &[u8], peer: u64) -> Result<ProjectDoc, String> {
        let doc = LoroDoc::new();
        doc.import(bytes)
            .map_err(|e| format!("not a project file, or damaged: {e}"))?;
        doc.set_peer_id(peer).map_err(|e| e.to_string())?;
        let mut d = ProjectDoc::wrap(doc);
        // Written by an older version, or by a copy that hadn't heard of
        // a deletion yet: check on the first save.
        d.scrub = true;
        Ok(d)
    }

    fn wrap(doc: LoroDoc) -> ProjectDoc {
        let mut undo = UndoManager::new(&doc);
        // One step per edit, however quickly they follow each other.
        undo.set_merge_interval(0);
        undo.set_max_undo_steps(500);
        undo.add_exclude_origin_prefix(QUIET);
        ProjectDoc {
            doc,
            undo,
            scrub: false,
            dropped: false,
            scrubbed: RefCell::new(None),
        }
    }

    /// The whole document, history included, for saving or sending,
    /// with the text of deleted comments, and the old text of changed
    /// ones, blanked out of the history.
    pub fn to_bytes(&self) -> Vec<u8> {
        if !self.scrub {
            return self.raw_bytes();
        }
        let vv = self.doc.oplog_vv();
        if let Some((at, bytes)) = &*self.scrubbed.borrow()
            && *at == vv
        {
            return bytes.clone();
        }
        let bytes = self.scrubbed_bytes().unwrap_or_else(|| self.raw_bytes());
        *self.scrubbed.borrow_mut() = Some((vv, bytes.clone()));
        bytes
    }

    /// Whether a comment has been deleted or changed (here or by someone
    /// else) since this was last asked. Saving should then not keep the
    /// save before as a backup, as it still holds the old text.
    pub fn take_dropped(&mut self) -> bool {
        std::mem::take(&mut self.dropped)
    }

    /// The document exactly as it is, history and all.
    pub fn raw_bytes(&self) -> Vec<u8> {
        self.doc.export(ExportMode::Snapshot).unwrap_or_default()
    }

    /// A copy with the history scrubbed, checked to show the same
    /// project. `None` if that fails, which should not happen.
    fn scrubbed_bytes(&self) -> Option<Vec<u8>> {
        let mut json = self
            .doc
            .export_json_updates_without_peer_compression(
                &VersionVector::new(),
                &self.doc.oplog_vv(),
            )
            .ok()?;
        let live: HashSet<ContainerID> = self.comment_maps().into_iter().collect();
        if scrub(&mut json, &self.comments().id(), &live) == 0 {
            return Some(self.raw_bytes());
        }
        let copy = LoroDoc::new();
        copy.import_json_updates(json).ok()?;
        (shown(&copy) == shown(&self.doc))
            .then(|| copy.export(ExportMode::Snapshot).ok())
            .flatten()
    }

    /// The containers of the comments the project shows now.
    fn comment_maps(&self) -> Vec<ContainerID> {
        let comments = self.comments();
        keys(&comments)
            .iter()
            .filter_map(|k| comments.get(k)?.into_container().ok()?.into_map().ok())
            .map(|m| m.id())
            .collect()
    }

    /// This document's version, to ask another copy for what is new.
    pub fn version(&self) -> Vec<u8> {
        self.doc.oplog_vv().encode()
    }

    /// Changes made since `version`, to send to another copy.
    pub fn updates_since(&self, version: &[u8]) -> Result<Vec<u8>, String> {
        let vv = VersionVector::decode(version).map_err(|e| e.to_string())?;
        self.doc
            .export(ExportMode::updates(&vv))
            .map_err(|e| e.to_string())
    }

    /// Merges changes or a whole document from another copy.
    pub fn merge(&mut self, bytes: &[u8]) -> Result<(), String> {
        let before = self.comments().get_deep_value();
        if let Err(e) = self.doc.import(bytes) {
            // A copy that was scrubbed, or wasn't when this one was, holds
            // different values for changes both have, which Loro refuses
            // as a clash. Take only the changes this copy lacks.
            if !matches!(e, LoroError::UsedOpID { .. }) {
                return Err(e.to_string());
            }
            let theirs = LoroDoc::new();
            theirs.import(bytes).map_err(|e| e.to_string())?;
            let new = theirs
                .export(ExportMode::updates(&self.doc.oplog_vv()))
                .map_err(|e| e.to_string())?;
            self.doc.import(&new).map_err(|e| e.to_string())?;
        }
        // Someone else may have deleted or changed a comment, or sent
        // history that still holds old text.
        if self.comments().get_deep_value() != before {
            self.scrub = true;
            self.dropped = true;
        } else if !self.comments().is_empty() {
            self.scrub = true;
        }
        Ok(())
    }

    /// Writes what differs from `p` as one undoable edit. True if
    /// anything did.
    pub fn set(&mut self, p: &Project) -> bool {
        let changed = self.write(p);
        self.doc.commit();
        changed
    }

    /// Like [`set`](Self::set), but not undoable: for bringing the
    /// document up to date with things that aren't edits, such as plugin
    /// settings read when saving.
    pub fn set_quietly(&mut self, p: &Project) -> bool {
        let changed = self.write(p);
        self.doc.set_next_commit_origin(QUIET);
        self.doc.commit();
        changed
    }

    pub fn can_undo(&self) -> bool {
        self.undo.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.undo.can_redo()
    }

    /// Undoes this user's last edit (not other people's). True if there
    /// was one.
    pub fn undo(&mut self) -> bool {
        self.undo.undo().unwrap_or(false)
    }

    pub fn redo(&mut self) -> bool {
        self.undo.redo().unwrap_or(false)
    }

    fn song(&self) -> LoroMap {
        self.doc.get_map("song")
    }

    fn tracks(&self) -> LoroMap {
        self.doc.get_map("tracks")
    }

    fn order(&self) -> LoroMovableList {
        self.doc.get_movable_list("order")
    }

    fn markers(&self) -> LoroMap {
        self.doc.get_map("markers")
    }

    fn notes(&self) -> LoroText {
        self.doc.get_text("notes")
    }

    /// Writes `who`'s practice settings where they differ. Not undoable:
    /// they change as you play (the tempo trainer, say), not as edits.
    /// True if anything changed.
    pub fn set_practice(&mut self, who: &str, p: &Practice) -> bool {
        let mut w = Writer::default();
        let m = w.child(&self.doc.get_map("practice"), who);
        w.put(&m, "speed_pct", p.speed_pct);
        w.put(&m, "semitones", p.semitones as i64);
        w.put(&m, "cents", p.cents as i64);
        w.put(&m, "looping", p.looping);
        let muted = w.fixed(&m, "muted");
        let keep: HashSet<&str> = p.muted.iter().map(String::as_str).collect();
        for id in keys(&muted) {
            if !keep.contains(id.as_str()) {
                w.delete(&muted, &id);
            }
        }
        for id in &p.muted {
            w.put(&muted, id, true);
        }
        let t = &p.trainer;
        w.put(&m, "trainer_on", t.on);
        w.put(&m, "trainer_step", t.step);
        w.put(&m, "trainer_every", t.every as i64);
        w.put(&m, "trainer_target", t.target);
        if w.changed {
            self.doc.set_next_commit_origin(QUIET);
            self.doc.commit();
        }
        w.changed
    }

    /// `who`'s practice settings, or `None` if they have none here yet.
    pub fn practice(&self, who: &str) -> Option<Practice> {
        let LoroValue::Map(all) = self.doc.get_map("practice").get_deep_value() else {
            return None;
        };
        let Some(LoroValue::Map(m)) = all.get(who) else {
            return None;
        };
        let d = Practice::default();
        let dt = TrainerSettings::default();
        let mut muted: Vec<String> = match m.get("muted") {
            Some(LoroValue::Map(ids)) => (ids.iter())
                .filter(|(_, v)| matches!(v, LoroValue::Bool(true)))
                .map(|(id, _)| id.to_string())
                .collect(),
            _ => Vec::new(),
        };
        muted.sort();
        Some(Practice {
            speed_pct: float(m, "speed_pct").unwrap_or(d.speed_pct),
            semitones: int(m, "semitones").map_or(d.semitones, |v| v as i32),
            cents: int(m, "cents").map_or(d.cents, |v| v as i32),
            looping: boolean(m, "looping").unwrap_or(d.looping),
            muted,
            trainer: TrainerSettings {
                on: boolean(m, "trainer_on").unwrap_or(dt.on),
                step: float(m, "trainer_step").unwrap_or(dt.step),
                every: int(m, "trainer_every").map_or(dt.every, |v| v.max(1) as u64),
                target: float(m, "trainer_target").unwrap_or(dt.target),
            },
        })
    }

    fn comments(&self) -> LoroMap {
        self.doc.get_map("comments")
    }

    fn write(&mut self, p: &Project) -> bool {
        let mut w = Writer::default();
        let song = self.song();
        w.put(&song, "rate", p.rate as i64);
        w.put(&song, "tempo", p.tempo_bpm);
        w.put(&song, "beats_per_bar", p.beats_per_bar as i64);
        w.put(&song, "loop_from", p.loop_bars.0);
        w.put(&song, "loop_to", p.loop_bars.1);
        w.put(&song, "reference_hz", p.reference_hz);

        let tracks = self.tracks();
        let keep: HashSet<&str> = p.tracks.iter().map(|t| t.id.as_str()).collect();
        for id in keys(&tracks) {
            if !keep.contains(id.as_str()) {
                w.delete(&tracks, &id);
            }
        }
        for t in &p.tracks {
            let m = w.child(&tracks, &t.id);
            write_track(&mut w, &m, t);
        }
        let ids: Vec<String> = p.tracks.iter().map(|t| t.id.clone()).collect();
        w.order(&self.order(), &ids);

        let markers = self.markers();
        let keep: HashSet<&str> = p.markers.iter().map(|m| m.id.as_str()).collect();
        for id in keys(&markers) {
            if !keep.contains(id.as_str()) {
                w.delete(&markers, &id);
            }
        }
        for mk in &p.markers {
            let m = w.child(&markers, &mk.id);
            w.put(&m, "bar", mk.bar);
            w.put(&m, "name", mk.name.as_str());
        }

        let notes = self.notes();
        if notes.to_string() != p.notes {
            let _ = notes.update(&p.notes, UpdateOptions::default());
            w.changed = true;
        }

        let comments = self.comments();
        let keep: HashSet<&str> = p.comments.iter().map(|c| c.id.as_str()).collect();
        for id in keys(&comments) {
            if !keep.contains(id.as_str()) {
                w.delete(&comments, &id);
                self.scrub = true;
                self.dropped = true;
            }
        }
        for c in &p.comments {
            let m = w.child(&comments, &c.id);
            let had = m.get("text").is_some();
            if w.put(&m, "text", c.text.as_str()) && had {
                self.scrub = true;
                self.dropped = true;
            }
            w.put(&m, "author", c.author.as_str());
            w.put(&m, "parent", opt_str(&c.parent));
            w.put(&m, "track", opt_str(&c.track));
            w.put(&m, "start", c.start);
            w.put(&m, "end", c.end.map_or(LoroValue::Null, LoroValue::I64));
            w.put(&m, "resolved", c.resolved);
            w.put(&m, "created", c.created);
        }
        w.changed
    }

    /// The project as it stands.
    pub fn get(&self) -> Project {
        let mut p = Project::default();
        let LoroValue::Map(root) = self.doc.get_deep_value() else {
            return p;
        };
        if let Some(LoroValue::Map(s)) = root.get("song") {
            p.rate = int(s, "rate").map_or(p.rate, |v| v as u32);
            p.tempo_bpm = float(s, "tempo").unwrap_or(p.tempo_bpm);
            p.beats_per_bar = int(s, "beats_per_bar").map_or(p.beats_per_bar, |v| v as u32);
            p.loop_bars = (
                int(s, "loop_from").unwrap_or(p.loop_bars.0),
                int(s, "loop_to").unwrap_or(p.loop_bars.1),
            );
            p.reference_hz = float(s, "reference_hz").unwrap_or(p.reference_hz);
        }
        let empty = LoroMapValue::default();
        let tracks = match root.get("tracks") {
            Some(LoroValue::Map(m)) => m,
            _ => &empty,
        };
        let mut order: Vec<String> = Vec::new();
        if let Some(LoroValue::List(ids)) = root.get("order") {
            for v in ids.iter() {
                if let LoroValue::String(s) = v
                    && tracks.contains_key(s.as_str())
                    && !order.iter().any(|o| o == s.as_str())
                {
                    order.push(s.to_string());
                }
            }
        }
        // Tracks missing from the order (added and reordered at once by
        // two people) go at the end.
        let mut rest: Vec<&String> = tracks.keys().filter(|k| !order.contains(k)).collect();
        rest.sort();
        order.extend(rest.into_iter().cloned());
        for id in order {
            if let Some(LoroValue::Map(t)) = tracks.get(&id) {
                p.tracks.push(read_track(&id, t));
            }
        }
        if let Some(LoroValue::Map(ms)) = root.get("markers") {
            for (id, m) in ms.iter() {
                let LoroValue::Map(m) = m else { continue };
                p.markers.push(Marker {
                    id: id.to_string(),
                    bar: int(m, "bar").unwrap_or(1),
                    name: string(m, "name").unwrap_or_default(),
                });
            }
        }
        p.markers
            .sort_by(|a, b| a.bar.cmp(&b.bar).then(a.id.cmp(&b.id)));
        if let Some(LoroValue::String(s)) = root.get("notes") {
            p.notes = s.to_string();
        }
        if let Some(LoroValue::Map(cs)) = root.get("comments") {
            for (id, c) in cs.iter() {
                let LoroValue::Map(c) = c else { continue };
                p.comments.push(Comment {
                    id: id.to_string(),
                    author: string(c, "author").unwrap_or_default(),
                    parent: string(c, "parent"),
                    track: string(c, "track"),
                    start: int(c, "start").unwrap_or(0),
                    end: int(c, "end"),
                    text: string(c, "text").unwrap_or_default(),
                    resolved: boolean(c, "resolved").unwrap_or(false),
                    created: int(c, "created").unwrap_or(0),
                });
            }
        }
        p.comments
            .sort_by(|a, b| a.created.cmp(&b.created).then(a.id.cmp(&b.id)));
        p
    }
}

/// Blanks, in `json` (a document's whole history), every value written
/// into a comment that the document no longer shows: all of a deleted
/// comment's fields, and the values a comment's fields had before they
/// were last changed. Comments are the maps put into `root`; `live` are
/// the ones still there. Returns how many values it blanked.
///
/// Only values are blanked, the same way `loro::json::redact` does, so
/// every change keeps its id and the scrubbed copy still merges with
/// copies that were never scrubbed: merging never brings the old text
/// back, as those changes are already there.
///
/// `json` must be exported without peer compression, so that peers and
/// container ids are written out in full.
fn scrub(json: &mut JsonSchema, root: &ContainerID, live: &HashSet<ContainerID>) -> usize {
    // Every comment there has ever been.
    let mut ever: HashSet<ContainerID> = HashSet::new();
    for ch in &json.changes {
        for op in &ch.ops {
            if op.container == *root
                && let JsonOpContent::Map(JsonMapOp::Insert {
                    value: LoroValue::Container(c),
                    ..
                }) = &op.content
            {
                ever.insert(c.clone());
            }
        }
    }
    // In a live comment, the write to each field that wins: the latest
    // by Lamport time, then by peer, as Loro's maps decide.
    let mut wins: HashMap<(&ContainerID, &str), (u32, u64)> = HashMap::new();
    for ch in &json.changes {
        for op in &ch.ops {
            if !live.contains(&op.container) {
                continue;
            }
            let key = match &op.content {
                JsonOpContent::Map(JsonMapOp::Insert { key, .. })
                | JsonOpContent::Map(JsonMapOp::Delete { key }) => key.as_str(),
                _ => continue,
            };
            let at = (ch.lamport + (op.counter - ch.id.counter) as u32, ch.id.peer);
            let w = wins.entry((&op.container, key)).or_insert(at);
            *w = (*w).max(at);
        }
    }
    let wins: HashMap<(ContainerID, String), (u32, u64)> = wins
        .into_iter()
        .map(|((c, k), v)| ((c.clone(), k.to_string()), v))
        .collect();
    let mut blanked = 0;
    for ch in &mut json.changes {
        let (lamport, counter, who) = (ch.lamport, ch.id.counter, ch.id.peer);
        for op in &mut ch.ops {
            if !ever.contains(&op.container) {
                continue;
            }
            let JsonOpContent::Map(JsonMapOp::Insert { key, value }) = &mut op.content else {
                continue;
            };
            if matches!(value, LoroValue::Null | LoroValue::Container(_)) {
                continue;
            }
            let at = (lamport + (op.counter - counter) as u32, who);
            let keep = live.contains(&op.container)
                && wins.get(&(op.container.clone(), key.clone())) == Some(&at);
            if !keep {
                *value = LoroValue::Null;
                blanked += 1;
            }
        }
    }
    blanked
}

/// What a document shows, leaving out root containers that are empty
/// (which a document lists only once something has asked for them).
fn shown(doc: &LoroDoc) -> Vec<(String, LoroValue)> {
    let LoroValue::Map(root) = doc.get_deep_value() else {
        return Vec::new();
    };
    let empty = |v: &LoroValue| match v {
        LoroValue::Map(m) => m.is_empty(),
        LoroValue::List(l) => l.is_empty(),
        LoroValue::String(s) => s.is_empty(),
        _ => false,
    };
    let mut all: Vec<(String, LoroValue)> = root
        .iter()
        .filter(|(_, v)| !empty(v))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    all.sort_by(|a, b| a.0.cmp(&b.0));
    all
}

fn opt_str(s: &Option<String>) -> LoroValue {
    s.as_deref().map_or(LoroValue::Null, LoroValue::from)
}

fn write_track(w: &mut Writer, m: &LoroMap, t: &Track) {
    w.put(m, "name", t.name.as_str());
    w.put(m, "gain_db", t.gain_db);
    w.put(m, "pan", t.pan);
    w.put(m, "mute", t.mute);
    w.put(m, "solo", t.solo);

    let clips = w.fixed(m, "clips");
    let keep: HashSet<&str> = t.clips.iter().map(|c| c.id.as_str()).collect();
    for id in keys(&clips) {
        if !keep.contains(id.as_str()) {
            w.delete(&clips, &id);
        }
    }
    for c in &t.clips {
        let cm = w.child(&clips, &c.id);
        w.put(&cm, "asset", c.asset.as_str());
        w.put(&cm, "name", c.name.as_str());
        w.put(&cm, "start", c.start);
        w.put(&cm, "offset", c.offset);
        w.put(&cm, "len", c.len);
        w.put(
            &cm,
            "take",
            c.take.map_or(LoroValue::Null, |t| LoroValue::I64(t as i64)),
        );
        w.put(&cm, "muted", c.muted);
        w.put(&cm, "seq", c.seq);
    }

    let fx = w.fixed(m, "fx");
    let keep: HashSet<&str> = t.effects.iter().map(|e| e.id.as_str()).collect();
    for id in keys(&fx) {
        if !keep.contains(id.as_str()) {
            w.delete(&fx, &id);
        }
    }
    for e in &t.effects {
        let em = w.child(&fx, &e.id);
        w.put(&em, "name", e.name.as_str());
        w.put(&em, "class", e.class.clone());
        w.put(&em, "bundle", e.bundle.as_str());
        w.put(&em, "bypass", e.bypass);
        if let Some(s) = &e.state {
            w.put(&em, "component", s.component.clone());
            w.put(
                &em,
                "controller",
                s.controller
                    .clone()
                    .map_or(LoroValue::Null, LoroValue::from),
            );
        }
    }
    let Ok(order_list) = m.ensure_mergeable_movable_list("fx_order") else {
        return;
    };
    let ids: Vec<String> = t.effects.iter().map(|e| e.id.clone()).collect();
    w.order(&order_list, &ids);
}

fn read_track(id: &str, t: &LoroMapValue) -> Track {
    let mut track = Track {
        id: id.to_string(),
        name: string(t, "name").unwrap_or_default(),
        gain_db: float(t, "gain_db").unwrap_or(0.0),
        pan: float(t, "pan").unwrap_or(0.0),
        mute: boolean(t, "mute").unwrap_or(false),
        solo: boolean(t, "solo").unwrap_or(false),
        clips: Vec::new(),
        effects: Vec::new(),
    };
    if let Some(LoroValue::Map(clips)) = t.get("clips") {
        for (cid, c) in clips.iter() {
            let LoroValue::Map(c) = c else { continue };
            track.clips.push(Clip {
                id: cid.to_string(),
                asset: string(c, "asset").unwrap_or_default(),
                name: string(c, "name").unwrap_or_default(),
                start: int(c, "start").unwrap_or(0),
                offset: int(c, "offset").unwrap_or(0),
                len: int(c, "len").unwrap_or(0),
                take: int(c, "take").map(|v| v as u32),
                muted: boolean(c, "muted").unwrap_or(false),
                seq: int(c, "seq").unwrap_or(0),
            });
        }
    }
    track
        .clips
        .sort_by(|a, b| a.seq.cmp(&b.seq).then(a.id.cmp(&b.id)));
    if let Some(LoroValue::Map(fx)) = t.get("fx") {
        let mut ids: Vec<String> = Vec::new();
        if let Some(LoroValue::List(order)) = t.get("fx_order") {
            for v in order.iter() {
                if let LoroValue::String(s) = v
                    && fx.contains_key(s.as_str())
                    && !ids.iter().any(|i| i == s.as_str())
                {
                    ids.push(s.to_string());
                }
            }
        }
        let mut rest: Vec<&String> = fx.keys().filter(|k| !ids.contains(k)).collect();
        rest.sort();
        ids.extend(rest.into_iter().cloned());
        for eid in ids {
            let Some(LoroValue::Map(e)) = fx.get(&eid) else {
                continue;
            };
            let state = binary(e, "component").map(|component| EffectState {
                component,
                controller: binary(e, "controller"),
            });
            track.effects.push(Effect {
                id: eid,
                name: string(e, "name").unwrap_or_default(),
                class: binary(e, "class").unwrap_or_default(),
                bundle: string(e, "bundle").unwrap_or_default(),
                bypass: boolean(e, "bypass").unwrap_or(false),
                state,
            });
        }
    }
    track
}

/// Writes values only where they differ, noting whether any did.
#[derive(Default)]
struct Writer {
    changed: bool,
}

impl Writer {
    /// True if it wrote.
    fn put(&mut self, m: &LoroMap, key: &str, v: impl Into<LoroValue>) -> bool {
        let v = v.into();
        let now = m.get(key).and_then(|x| x.into_value().ok());
        if now.as_ref() != Some(&v) {
            let _ = m.insert(key, v);
            self.changed = true;
            return true;
        }
        false
    }

    fn delete(&mut self, m: &LoroMap, key: &str) {
        let _ = m.delete(key);
        self.changed = true;
    }

    /// The map at `key`, made if missing.
    fn child(&mut self, m: &LoroMap, key: &str) -> LoroMap {
        match m
            .get(key)
            .and_then(|v| v.into_container().ok()?.into_map().ok())
        {
            Some(c) => c,
            None => {
                self.changed = true;
                m.insert_container(key, LoroMap::new())
                    .unwrap_or_else(|_| LoroMap::new())
            }
        }
    }

    /// The map at a fixed `key` in every track, made if missing. Two
    /// people making it at once get one map, not one each.
    fn fixed(&mut self, m: &LoroMap, key: &str) -> LoroMap {
        if m.get(key).is_none() {
            self.changed = true;
        }
        m.ensure_mergeable_map(key)
            .unwrap_or_else(|_| LoroMap::new())
    }

    /// Makes the list hold exactly `want`, in order, with as few moves
    /// as it takes.
    fn order(&mut self, list: &LoroMovableList, want: &[String]) {
        let wanted: HashSet<&str> = want.iter().map(String::as_str).collect();
        let mut seen = HashSet::new();
        let mut i = 0;
        while i < list.len() {
            match str_at(list, i) {
                Some(s) if wanted.contains(s.as_str()) && seen.insert(s.clone()) => i += 1,
                _ => {
                    let _ = list.delete(i, 1);
                    self.changed = true;
                }
            }
        }
        for id in want {
            if !seen.contains(id) {
                let _ = list.push(id.as_str());
                self.changed = true;
            }
        }
        for (to, id) in want.iter().enumerate() {
            let from = (0..list.len()).find(|&j| str_at(list, j).as_deref() == Some(id.as_str()));
            if let Some(from) = from
                && from != to
            {
                let _ = list.mov(from, to);
                self.changed = true;
            }
        }
    }
}

fn keys(m: &LoroMap) -> Vec<String> {
    m.keys().map(|k| k.to_string()).collect()
}

fn str_at(list: &LoroMovableList, i: usize) -> Option<String> {
    match list.get(i)?.into_value().ok()? {
        LoroValue::String(s) => Some(s.to_string()),
        _ => None,
    }
}

fn int(m: &LoroMapValue, k: &str) -> Option<i64> {
    match m.get(k) {
        Some(LoroValue::I64(v)) => Some(*v),
        Some(LoroValue::Double(v)) => Some(*v as i64),
        _ => None,
    }
}

fn float(m: &LoroMapValue, k: &str) -> Option<f64> {
    match m.get(k) {
        Some(LoroValue::Double(v)) => Some(*v),
        Some(LoroValue::I64(v)) => Some(*v as f64),
        _ => None,
    }
}

fn boolean(m: &LoroMapValue, k: &str) -> Option<bool> {
    match m.get(k) {
        Some(LoroValue::Bool(v)) => Some(*v),
        _ => None,
    }
}

fn string(m: &LoroMapValue, k: &str) -> Option<String> {
    match m.get(k) {
        Some(LoroValue::String(s)) => Some(s.to_string()),
        _ => None,
    }
}

fn binary(m: &LoroMapValue, k: &str) -> Option<Vec<u8>> {
    match m.get(k) {
        Some(LoroValue::Binary(b)) => Some(b.to_vec()),
        _ => None,
    }
}
