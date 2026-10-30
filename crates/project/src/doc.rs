// SPDX-License-Identifier: GPL-3.0-or-later
//! The project document on Loro.
//!
//! Layout: a `song` map of settings, a `tracks` map from id to track,
//! with the track order in the movable list `order`, a `markers` map,
//! `tempo_changes` and `meter_changes` maps from id to change, a
//! `hit_points` map from id to hit point, and
//! the lesson notes as text, a `tabs` map from id to tab block, an
//! `assignments` map from id to assignment, and a
//! `comments` map from id to comment, and an `edit_groups` map from id
//! to group, whose tracks are a map from track id to `true` so two
//! people adding tracks at once keep both. A
//! track is a map of its settings, a `clips`
//! map from id to clip, a `note_clips` map from id to note clip (each
//! with a `notes` map from id to note), an `instrument` map, an `fx`
//! map from id to effect and the effect
//! order in the movable list `fx_order`, and its frozen audio, if any, as
//! one `frozen` value (so two renders saved at once never mix). Every
//! other field is its own map entry,
//! so two people changing different fields of the same clip both keep
//! their change. A `practice` map holds each person's practice settings
//! under their own key, which only they write; the practice log and the
//! list of lessons are in `lessons.rs`.
//!
//! Deleting a comment, or changing its text, has to take the old words
//! out of the file too, not just out of the project as it stands: the
//! document keeps its whole history, so they could otherwise be read
//! back. [`ProjectDoc::to_bytes`] blanks every value that comments no
//! longer show in the copy it writes (see [`scrub`]). Tab blocks are laid
//! out the same way, one value per field, so the same scrub could cover
//! them; for now their old text, and the notes', stays in the history.
//!
//! Named versions of the project, and restoring them, are in
//! `snapshots.rs`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use loro::{
    ContainerID, ContainerTrait, ExportMode, JsonMapOp, JsonOpContent, JsonSchema, LoroDoc,
    LoroError, LoroMap, LoroMapValue, LoroMovableList, LoroText, LoroValue, UndoManager,
    UpdateOptions, VersionVector,
};

use crate::identity::Me;
use crate::{
    Articulation, Assignment, Clip, Comment, Comp, CompPart, ControlGroup, CueLevel, CueMix, Curve,
    EditGroup, Effect, EffectState, Fade, Frozen, FrozenClip, GainPoint, HitPoint, Marker,
    MeterChange, Note, NoteClip, Practice, Project, Send, TabBlock, Technique, TempoChange,
    TimecodeSetup, Track, TrackKind, TrainerSettings, is_asset_name,
};

/// Commits with this origin (plugin state written at save time) are left
/// out of undo.
pub(crate) const QUIET: &str = "quiet";

pub struct ProjectDoc {
    pub(crate) doc: LoroDoc,
    undo: UndoManager,
    /// Whether the history may hold comment text that comments no longer
    /// show, so saving has to scrub it.
    scrub: bool,
    /// Whether a comment was deleted or changed since
    /// [`take_dropped`](Self::take_dropped) was last asked.
    dropped: bool,
    /// The last scrubbed copy, and the version it was made at.
    scrubbed: RefCell<Option<(VersionVector, Vec<u8>)>>,
    /// Who is editing, once signed in (see [`crate::identity`]).
    pub(crate) me: Option<Me>,
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

    pub(crate) fn wrap(doc: LoroDoc) -> ProjectDoc {
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
            me: None,
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

    /// Merges changes or a whole document from another copy, unless
    /// they include changes from people not allowed to make them
    /// ([`check`](Self::check)), in which case nothing is merged.
    pub fn merge(&mut self, bytes: &[u8]) -> Result<(), String> {
        let check = self.check(bytes)?;
        if !check.ok() {
            return Err(format!("not merged: {}", check.reason()));
        }
        let before = self.comments().get_deep_value();
        import_lenient(&self.doc, bytes)?;
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

    fn tabs(&self) -> LoroMap {
        self.doc.get_map("tabs")
    }

    fn write(&mut self, p: &Project) -> bool {
        let mut w = Writer::default();
        let song = self.song();
        w.put(&song, "rate", p.rate as i64);
        w.put(&song, "tempo", p.tempo_bpm);
        w.put(&song, "beats_per_bar", p.beats_per_bar as i64);
        w.put(&song, "beat_unit", p.beat_unit as i64);
        w.put(&song, "loop_from", p.loop_bars.0);
        w.put(&song, "loop_to", p.loop_bars.1);
        w.put(&song, "reference_hz", p.reference_hz);
        // Timecode only once a project has set it up, so older projects
        // aren't changed by being opened.
        if p.timecode != TimecodeSetup::default() || song.get("tc_rate").is_some() {
            w.put(&song, "tc_rate", p.timecode.rate.as_str());
            w.put(&song, "tc_start", p.timecode.start.as_str());
            w.put(&song, "tc_offset", p.timecode.offset);
            w.put(&song, "tc_shown", p.timecode.shown);
        }

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

        // Control groups: an entry only once a project has one, so older
        // projects gain nothing.
        if !p.control_groups.is_empty() || self.doc.get_by_str_path("groups").is_some() {
            let groups = self.doc.get_map("groups");
            let keep: HashSet<&str> = p.control_groups.iter().map(|g| g.id.as_str()).collect();
            for id in keys(&groups) {
                if !keep.contains(id.as_str()) {
                    w.delete(&groups, &id);
                }
            }
            for g in &p.control_groups {
                let m = w.child(&groups, &g.id);
                w.put(&m, "name", g.name.as_str());
                w.put(&m, "gain_db", g.gain_db);
                w.put(&m, "mute", g.mute);
            }
            let ids: Vec<String> = p.control_groups.iter().map(|g| g.id.clone()).collect();
            w.order(&self.doc.get_movable_list("group_order"), &ids);
        }

        // Headphone mixes, likewise only once a project has one.
        if !p.cue_mixes.is_empty() || self.doc.get_by_str_path("cue_mixes").is_some() {
            write_cue_mixes(&mut w, &self.doc, &p.cue_mixes);
        }

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

        let hits = self.doc.get_map("hit_points");
        let keep: HashSet<&str> = p.hit_points.iter().map(|h| h.id.as_str()).collect();
        for id in keys(&hits) {
            if !keep.contains(id.as_str()) {
                w.delete(&hits, &id);
            }
        }
        for h in &p.hit_points {
            let m = w.child(&hits, &h.id);
            w.put(&m, "at", h.at);
            w.put(&m, "name", h.name.as_str());
        }

        let tempos = self.doc.get_map("tempo_changes");
        let keep: HashSet<&str> = p.tempo_changes.iter().map(|c| c.id.as_str()).collect();
        for id in keys(&tempos) {
            if !keep.contains(id.as_str()) {
                w.delete(&tempos, &id);
            }
        }
        for c in &p.tempo_changes {
            let m = w.child(&tempos, &c.id);
            w.put(&m, "bar", c.bar);
            w.put(&m, "beat", c.beat as i64);
            w.put(&m, "bpm", c.bpm);
            w.put(&m, "glide", c.glide);
        }

        let meters = self.doc.get_map("meter_changes");
        let keep: HashSet<&str> = p.meter_changes.iter().map(|c| c.id.as_str()).collect();
        for id in keys(&meters) {
            if !keep.contains(id.as_str()) {
                w.delete(&meters, &id);
            }
        }
        for c in &p.meter_changes {
            let m = w.child(&meters, &c.id);
            w.put(&m, "bar", c.bar);
            w.put(&m, "beats", c.beats as i64);
            w.put(&m, "unit", c.unit as i64);
        }

        let notes = self.notes();
        if notes.to_string() != p.notes {
            let _ = notes.update(&p.notes, UpdateOptions::default());
            w.changed = true;
        }

        let tabs = self.tabs();
        let keep: HashSet<&str> = p.tabs.iter().map(|t| t.id.as_str()).collect();
        for id in keys(&tabs) {
            if !keep.contains(id.as_str()) {
                w.delete(&tabs, &id);
            }
        }
        for t in &p.tabs {
            let m = w.child(&tabs, &t.id);
            w.put(&m, "marker", opt_str(&t.marker));
            w.put(&m, "title", t.title.as_str());
            w.put(&m, "text", t.text.as_str());
            w.put(&m, "author", t.author.as_str());
            w.put(&m, "created", t.created);
        }

        // Assignments: an entry only once a project has one.
        if !p.assignments.is_empty() || self.doc.get_by_str_path("assignments").is_some() {
            let all = self.doc.get_map("assignments");
            let keep: HashSet<&str> = p.assignments.iter().map(|a| a.id.as_str()).collect();
            for id in keys(&all) {
                if !keep.contains(id.as_str()) {
                    w.delete(&all, &id);
                }
            }
            for a in &p.assignments {
                let m = w.child(&all, &a.id);
                w.put(&m, "from", a.bars.0);
                w.put(&m, "to", a.bars.1);
                w.put(&m, "bpm", a.bpm);
                w.put(&m, "due", a.due);
                w.put(&m, "note", a.note.as_str());
                w.put(&m, "lesson", opt_str(&a.lesson));
                w.put(&m, "author", a.author.as_str());
                w.put(&m, "created", a.created);
            }
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

        let groups = self.doc.get_map("edit_groups");
        let keep: HashSet<&str> = p.edit_groups.iter().map(|g| g.id.as_str()).collect();
        for id in keys(&groups) {
            if !keep.contains(id.as_str()) {
                w.delete(&groups, &id);
            }
        }
        for g in &p.edit_groups {
            let m = w.child(&groups, &g.id);
            w.put(&m, "name", g.name.as_str());
            w.put(&m, "active", g.active);
            w.put(&m, "created", g.created);
            let tm = w.fixed(&m, "tracks");
            let keep: HashSet<&str> = g.tracks.iter().map(String::as_str).collect();
            for k in keys(&tm) {
                if !keep.contains(k.as_str()) {
                    w.delete(&tm, &k);
                }
            }
            for t in &g.tracks {
                w.put(&tm, t, true);
            }
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
            p.beat_unit = int(s, "beat_unit").map_or(p.beat_unit, |v| v as u32);
            p.loop_bars = (
                int(s, "loop_from").unwrap_or(p.loop_bars.0),
                int(s, "loop_to").unwrap_or(p.loop_bars.1),
            );
            p.reference_hz = float(s, "reference_hz").unwrap_or(p.reference_hz);
            let tc = &mut p.timecode;
            tc.rate = string(s, "tc_rate").unwrap_or(tc.rate.clone());
            tc.start = string(s, "tc_start").unwrap_or(tc.start.clone());
            tc.offset = int(s, "tc_offset").unwrap_or(tc.offset);
            tc.shown = boolean(s, "tc_shown").unwrap_or(tc.shown);
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
        // Routes to tracks that are gone go to the master; sends come in
        // the order of the buses they go to.
        let ids: Vec<String> = p.tracks.iter().map(|t| t.id.clone()).collect();
        let place = |id: &str| ids.iter().position(|i| i == id);
        for t in &mut p.tracks {
            let own = t.id.clone();
            let gone =
                |id: &Option<String>| id.as_ref().is_some_and(|i| *i == own || place(i).is_none());
            if gone(&t.parent) {
                t.parent = None;
            }
            if gone(&t.output) {
                t.output = None;
            }
            t.sends.retain(|s| s.to != own && place(&s.to).is_some());
            t.sends.sort_by_key(|s| place(&s.to));
            for e in &mut t.effects {
                if gone(&e.sidechain) {
                    e.sidechain = None;
                }
            }
        }
        if let Some(LoroValue::Map(gs)) = root.get("groups") {
            let mut order: Vec<String> = Vec::new();
            if let Some(LoroValue::List(ids)) = root.get("group_order") {
                for v in ids.iter() {
                    if let LoroValue::String(s) = v
                        && gs.contains_key(s.as_str())
                        && !order.iter().any(|o| o == s.as_str())
                    {
                        order.push(s.to_string());
                    }
                }
            }
            let mut rest: Vec<&String> = gs.keys().filter(|k| !order.contains(k)).collect();
            rest.sort();
            order.extend(rest.into_iter().cloned());
            for id in order {
                let Some(LoroValue::Map(g)) = gs.get(&id) else {
                    continue;
                };
                p.control_groups.push(ControlGroup {
                    id,
                    name: string(g, "name").unwrap_or_default(),
                    gain_db: float(g, "gain_db").filter(|v| v.is_finite()).unwrap_or(0.0),
                    mute: boolean(g, "mute").unwrap_or(false),
                });
            }
            let known: HashSet<&str> = p.control_groups.iter().map(|g| g.id.as_str()).collect();
            for t in &mut p.tracks {
                if t.group
                    .as_ref()
                    .is_some_and(|g| !known.contains(g.as_str()))
                {
                    t.group = None;
                }
            }
        } else {
            for t in &mut p.tracks {
                t.group = None;
            }
        }
        p.cue_mixes = read_cue_mixes(&root, &p.tracks);
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
        if let Some(LoroValue::Map(hs)) = root.get("hit_points") {
            for (id, h) in hs.iter() {
                let LoroValue::Map(h) = h else { continue };
                p.hit_points.push(HitPoint {
                    id: id.to_string(),
                    at: int(h, "at").unwrap_or(0),
                    name: string(h, "name").unwrap_or_default(),
                });
            }
        }
        p.hit_points
            .sort_by(|a, b| a.at.cmp(&b.at).then(a.id.cmp(&b.id)));
        if let Some(LoroValue::Map(cs)) = root.get("tempo_changes") {
            for (id, c) in cs.iter() {
                let LoroValue::Map(c) = c else { continue };
                p.tempo_changes.push(TempoChange {
                    id: id.to_string(),
                    bar: int(c, "bar").unwrap_or(1),
                    beat: int(c, "beat").map_or(1, |v| v.max(1) as u32),
                    bpm: float(c, "bpm").unwrap_or(p.tempo_bpm),
                    glide: boolean(c, "glide").unwrap_or(false),
                });
            }
        }
        p.tempo_changes
            .sort_by(|a, b| (a.bar, a.beat, &a.id).cmp(&(b.bar, b.beat, &b.id)));
        if let Some(LoroValue::Map(cs)) = root.get("meter_changes") {
            for (id, c) in cs.iter() {
                let LoroValue::Map(c) = c else { continue };
                p.meter_changes.push(MeterChange {
                    id: id.to_string(),
                    bar: int(c, "bar").unwrap_or(1),
                    beats: int(c, "beats").map_or(4, |v| v.max(1) as u32),
                    unit: int(c, "unit").map_or(4, |v| v as u32),
                });
            }
        }
        p.meter_changes
            .sort_by(|a, b| a.bar.cmp(&b.bar).then(a.id.cmp(&b.id)));
        if let Some(LoroValue::String(s)) = root.get("notes") {
            p.notes = s.to_string();
        }
        if let Some(LoroValue::Map(ts)) = root.get("tabs") {
            for (id, t) in ts.iter() {
                let LoroValue::Map(t) = t else { continue };
                p.tabs.push(TabBlock {
                    id: id.to_string(),
                    marker: string(t, "marker"),
                    title: string(t, "title").unwrap_or_default(),
                    text: string(t, "text").unwrap_or_default(),
                    author: string(t, "author").unwrap_or_default(),
                    created: int(t, "created").unwrap_or(0),
                });
            }
        }
        p.tabs
            .sort_by(|a, b| a.created.cmp(&b.created).then(a.id.cmp(&b.id)));
        if let Some(LoroValue::Map(all)) = root.get("assignments") {
            for (id, a) in all.iter() {
                let LoroValue::Map(a) = a else { continue };
                let from = int(a, "from").unwrap_or(1).max(1);
                p.assignments.push(Assignment {
                    id: id.to_string(),
                    bars: (from, int(a, "to").unwrap_or(from + 1).max(from + 1)),
                    bpm: float(a, "bpm").unwrap_or(p.tempo_bpm),
                    due: int(a, "due").unwrap_or(0),
                    note: string(a, "note").unwrap_or_default(),
                    lesson: string(a, "lesson"),
                    author: string(a, "author").unwrap_or_default(),
                    created: int(a, "created").unwrap_or(0),
                });
            }
        }
        p.assignments
            .sort_by(|a, b| a.created.cmp(&b.created).then(a.id.cmp(&b.id)));
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
        if let Some(LoroValue::Map(gs)) = root.get("edit_groups") {
            for (id, g) in gs.iter() {
                let LoroValue::Map(g) = g else { continue };
                let mut tracks: Vec<String> = match g.get("tracks") {
                    Some(LoroValue::Map(ts)) => (ts.iter())
                        .filter(|(_, v)| matches!(v, LoroValue::Bool(true)))
                        .map(|(k, _)| k.to_string())
                        .collect(),
                    _ => Vec::new(),
                };
                tracks.sort();
                p.edit_groups.push(EditGroup {
                    id: id.to_string(),
                    name: string(g, "name").unwrap_or_default(),
                    tracks,
                    active: boolean(g, "active").unwrap_or(true),
                    created: int(g, "created").unwrap_or(0),
                });
            }
        }
        p.edit_groups
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

pub(crate) fn opt_str(s: &Option<String>) -> LoroValue {
    s.as_deref().map_or(LoroValue::Null, LoroValue::from)
}

/// Imports another copy into `doc`. A copy that was scrubbed, or wasn't
/// when this one was, holds different values for changes both have,
/// which Loro refuses as a clash: then only the changes `doc` lacks are
/// taken.
pub(crate) fn import_lenient(doc: &LoroDoc, bytes: &[u8]) -> Result<(), String> {
    if let Err(e) = doc.import(bytes) {
        if !matches!(e, LoroError::UsedOpID { .. }) {
            return Err(e.to_string());
        }
        let theirs = LoroDoc::new();
        theirs.import(bytes).map_err(|e| e.to_string())?;
        let new = theirs
            .export(ExportMode::updates(&doc.oplog_vv()))
            .map_err(|e| e.to_string())?;
        doc.import(&new).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn write_track(w: &mut Writer, m: &LoroMap, t: &Track) {
    w.put(m, "name", t.name.as_str());
    w.put(m, "gain_db", t.gain_db);
    w.put(m, "pan", t.pan);
    w.put(m, "mute", t.mute);
    w.put(m, "solo", t.solo);
    // Routing: plain audio tracks to the master, as every track was
    // before buses, gain no new entries.
    let plain = |k: &str, unset: bool| unset && m.get(k).is_none();
    if !plain("kind", t.kind == TrackKind::Audio) {
        w.put(m, "kind", t.kind.name());
    }
    for (k, v) in [
        ("parent", &t.parent),
        ("output", &t.output),
        ("group", &t.group),
    ] {
        if !plain(k, v.is_none()) {
            w.put(m, k, v.clone().map_or(LoroValue::Null, LoroValue::from));
        }
    }
    if !plain("sends", t.sends.is_empty()) {
        let sends = w.fixed(m, "sends");
        let keep: HashSet<&str> = t.sends.iter().map(|s| s.to.as_str()).collect();
        for id in keys(&sends) {
            if !keep.contains(id.as_str()) {
                w.delete(&sends, &id);
            }
        }
        for s in &t.sends {
            let sm = w.child(&sends, &s.to);
            w.put(&sm, "gain_db", s.gain_db);
            w.put(&sm, "pre_fader", s.pre_fader);
            w.put(&sm, "mute", s.mute);
        }
    }

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
        if c.comp || cm.get("comp").is_some() {
            w.put(&cm, "comp", c.comp);
        }
        // Clips at their plain level, as all were before fades, gain no
        // new entries.
        let plain = |k: &str, unset: bool| unset && cm.get(k).is_none();
        if !plain("gain_db", c.gain_db == 0.0) {
            w.put(&cm, "gain_db", c.gain_db);
        }
        for (k, f) in [("fade_in", c.fade_in), ("fade_out", c.fade_out)] {
            if !plain(k, f.len == 0) {
                w.put(&cm, k, f.len);
                w.put(&cm, &format!("{k}_curve"), f.curve.name());
            }
        }
        if !plain("envelope", c.envelope.is_empty()) {
            let points: Vec<LoroValue> = (c.envelope.iter())
                .map(|p| LoroValue::List(vec![p.at.into(), p.db.into()].into()))
                .collect();
            w.put(&cm, "envelope", LoroValue::List(points.into()));
        }
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
        write_effect(w, &em, e);
    }
    // The instrument: one map, like an effect's. Tracks that never had
    // one gain no entry.
    match &t.instrument {
        Some(inst) => {
            let im = w.child(m, "instrument");
            w.put(&im, "id", inst.id.as_str());
            write_effect(w, &im, inst);
        }
        None if m.get("instrument").is_some() => w.delete(m, "instrument"),
        None => {}
    }
    write_note_clips(w, m, t);
    write_comps(w, m, t);
    match &t.frozen {
        Some(f) => {
            w.put(m, "frozen", frozen_value(f));
        }
        // Projects from before frozen audio have no entry: don't add one.
        None if m.get("frozen").is_some() => {
            w.put(m, "frozen", LoroValue::Null);
        }
        None => {}
    }
    // Only tracks ever frozen gain an entry.
    if t.freeze || m.get("freeze").is_some() {
        w.put(m, "freeze", t.freeze);
    }
    let Ok(order_list) = m.ensure_mergeable_movable_list("fx_order") else {
        return;
    };
    let ids: Vec<String> = t.effects.iter().map(|e| e.id.clone()).collect();
    w.order(&order_list, &ids);
}

/// Writes an effect's (or instrument's) fields into its map.
fn write_effect(w: &mut Writer, em: &LoroMap, e: &Effect) {
    w.put(em, "name", e.name.as_str());
    w.put(em, "class", e.class.clone());
    w.put(em, "bundle", e.bundle.as_str());
    w.put(em, "bypass", e.bypass);
    if !e.builtin.is_empty() {
        w.put(em, "builtin", e.builtin.as_str());
        // One entry per setting, so two people changing different
        // settings at once keep both.
        let pm = w.child(em, "params");
        let keep: HashSet<&str> = e.params.iter().map(|(k, _)| k.as_str()).collect();
        for k in keys(&pm) {
            if !keep.contains(k.as_str()) {
                w.delete(&pm, &k);
            }
        }
        for (k, v) in &e.params {
            w.put(&pm, k, *v);
        }
        if !e.response.is_empty() || em.get("response").is_some() {
            w.put(em, "response", e.response.as_str());
        }
    }
    if e.sidechain.is_some() || em.get("sidechain").is_some() {
        w.put(
            em,
            "sidechain",
            e.sidechain.clone().map_or(LoroValue::Null, LoroValue::from),
        );
    }
    if e.container.is_some() || em.get("container").is_some() {
        w.put(
            em,
            "container",
            e.container.clone().map_or(LoroValue::Null, LoroValue::from),
        );
        w.put(em, "branch", e.branch as i64);
    }
    if !e.outputs.is_empty() || em.get("outputs").is_some() {
        let outs: Vec<LoroValue> = e.outputs.iter().map(|&o| (o as i64).into()).collect();
        w.put(em, "outputs", LoroValue::List(outs.into()));
    }
    if let Some(s) = &e.state {
        w.put(em, "component", s.component.clone());
        w.put(
            em,
            "controller",
            s.controller
                .clone()
                .map_or(LoroValue::Null, LoroValue::from),
        );
    }
}

/// A track's note clips: a `note_clips` map from id to clip, each a map
/// of its settings and a `notes` map from note id to note, one entry per
/// field, so two people editing different notes of one clip keep both.
/// Tracks without notes gain no entries.
fn write_note_clips(w: &mut Writer, m: &LoroMap, t: &Track) {
    if t.note_clips.is_empty() && m.get("note_clips").is_none() {
        return;
    }
    let clips = w.fixed(m, "note_clips");
    let keep: HashSet<&str> = t.note_clips.iter().map(|c| c.id.as_str()).collect();
    for id in keys(&clips) {
        if !keep.contains(id.as_str()) {
            w.delete(&clips, &id);
        }
    }
    for c in &t.note_clips {
        let cm = w.child(&clips, &c.id);
        w.put(&cm, "name", c.name.as_str());
        w.put(&cm, "start", c.start);
        w.put(&cm, "len", c.len);
        w.put(&cm, "offset", c.offset);
        w.put(&cm, "muted", c.muted);
        if c.take.is_some() || cm.get("take").is_some() {
            w.put(
                &cm,
                "take",
                c.take.map_or(LoroValue::Null, |t| LoroValue::I64(t as i64)),
            );
        }
        let notes = w.fixed(&cm, "notes");
        let keep: HashSet<&str> = c.notes.iter().map(|n| n.id.as_str()).collect();
        for id in keys(&notes) {
            if !keep.contains(id.as_str()) {
                w.delete(&notes, &id);
            }
        }
        for n in &c.notes {
            let nm = w.child(&notes, &n.id);
            w.put(&nm, "start", n.start);
            w.put(&nm, "len", n.len);
            w.put(&nm, "pitch", n.pitch as i64);
            w.put(&nm, "velocity", n.velocity as i64);
            // Plain notes, as most are, gain no entries for the rest.
            let plain = |k: &str, unset: bool| unset && nm.get(k).is_none();
            if !plain("channel", n.channel == 0) {
                w.put(&nm, "channel", n.channel as i64);
            }
            for (k, v) in [("string", n.string), ("fret", n.fret)] {
                if !plain(k, v.is_none()) {
                    w.put(
                        &nm,
                        k,
                        v.map_or(LoroValue::Null, |v| LoroValue::I64(v as i64)),
                    );
                }
            }
            if !plain("bend", n.bend == 0.0) {
                w.put(&nm, "bend", n.bend);
            }
            if !plain("techniques", n.techniques.is_empty()) {
                let list: Vec<LoroValue> = (n.techniques.iter()).map(|t| t.name().into()).collect();
                w.put(&nm, "techniques", LoroValue::List(list.into()));
            }
            if !plain("articulation", n.articulation.is_none()) {
                w.put(
                    &nm,
                    "articulation",
                    n.articulation.map_or(LoroValue::Null, |a| a.name().into()),
                );
            }
            if !plain("chance", n.chance == 1.0) {
                w.put(&nm, "chance", n.chance);
            }
        }
    }
}

/// A track's note clips, in time order, each with its notes in order.
fn read_note_clips(t: &LoroMapValue) -> Vec<NoteClip> {
    let Some(LoroValue::Map(clips)) = t.get("note_clips") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (cid, c) in clips.iter() {
        let LoroValue::Map(c) = c else { continue };
        let mut clip = NoteClip::new(
            cid.to_string(),
            string(c, "name").unwrap_or_default(),
            int(c, "start").unwrap_or(0).max(0),
            int(c, "len").unwrap_or(1),
        );
        clip.offset = int(c, "offset").unwrap_or(0).max(0);
        clip.muted = boolean(c, "muted").unwrap_or(false);
        clip.take = int(c, "take").and_then(|v| u32::try_from(v).ok());
        if let Some(LoroValue::Map(notes)) = c.get("notes") {
            for (nid, n) in notes.iter() {
                let LoroValue::Map(n) = n else { continue };
                let Some(pitch) = int(n, "pitch").and_then(|p| u8::try_from(p).ok()) else {
                    continue;
                };
                if pitch > 127 {
                    continue;
                }
                let mut note = Note::new(
                    int(n, "start").unwrap_or(0).max(0),
                    int(n, "len").unwrap_or(1),
                    pitch,
                    int(n, "velocity").unwrap_or(100).clamp(1, 127) as u8,
                );
                note.id = nid.to_string();
                note.channel = int(n, "channel").unwrap_or(0).clamp(0, 15) as u8;
                note.string = int(n, "string").and_then(|v| u8::try_from(v).ok());
                note.fret = int(n, "fret").and_then(|v| u8::try_from(v).ok());
                note.bend = float(n, "bend").filter(|b| b.is_finite()).unwrap_or(0.0);
                if let Some(LoroValue::List(l)) = n.get("techniques") {
                    note.techniques = (l.iter())
                        .filter_map(|v| match v {
                            LoroValue::String(s) => Technique::from_name(s),
                            _ => None,
                        })
                        .collect();
                }
                note.articulation =
                    string(n, "articulation").and_then(|a| Articulation::from_name(&a));
                note.chance = float(n, "chance")
                    .filter(|c| c.is_finite())
                    .unwrap_or(1.0)
                    .clamp(0.0, 1.0);
                clip.notes.push(note);
            }
        }
        clip.sort_notes();
        out.push(clip);
    }
    out.sort_by(|a, b| (a.start, a.id.as_str()).cmp(&(b.start, b.id.as_str())));
    out
}

/// A track's comps: a `comps` map from id to its name and parts, each
/// part a list of take, start and end, and `comp`, the one that plays.
/// Tracks never comped gain no entries.
fn write_cue_mixes(w: &mut Writer, doc: &LoroDoc, mixes: &[CueMix]) {
    let all = doc.get_map("cue_mixes");
    let keep: HashSet<&str> = mixes.iter().map(|c| c.id.as_str()).collect();
    for id in keys(&all) {
        if !keep.contains(id.as_str()) {
            w.delete(&all, &id);
        }
    }
    for c in mixes {
        let m = w.child(&all, &c.id);
        w.put(&m, "name", c.name.as_str());
        w.put(&m, "gain_db", c.gain_db);
        w.put(&m, "mute", c.mute);
        w.put(&m, "follow_main", c.follow_main);
        w.put(&m, "click", c.click_db.is_some());
        if let Some(db) = c.click_db {
            w.put(&m, "click_db", db);
        }
        w.put(&m, "talkback", c.talkback);
        let levels = w.fixed(&m, "levels");
        let keep: HashSet<&str> = c.levels.iter().map(|l| l.track.as_str()).collect();
        for id in keys(&levels) {
            if !keep.contains(id.as_str()) {
                w.delete(&levels, &id);
            }
        }
        for l in &c.levels {
            let lm = w.child(&levels, &l.track);
            w.put(&lm, "gain_db", l.gain_db);
            w.put(&lm, "pan", l.pan);
            w.put(&lm, "mute", l.mute);
        }
    }
    let ids: Vec<String> = mixes.iter().map(|c| c.id.clone()).collect();
    w.order(&doc.get_movable_list("cue_order"), &ids);
}

/// The headphone mixes, in order, without levels for tracks that are
/// gone.
fn read_cue_mixes(root: &LoroMapValue, tracks: &[Track]) -> Vec<CueMix> {
    let Some(LoroValue::Map(all)) = root.get("cue_mixes") else {
        return Vec::new();
    };
    let mut order: Vec<String> = Vec::new();
    if let Some(LoroValue::List(ids)) = root.get("cue_order") {
        for v in ids.iter() {
            if let LoroValue::String(s) = v
                && all.contains_key(s.as_str())
                && !order.iter().any(|o| o == s.as_str())
            {
                order.push(s.to_string());
            }
        }
    }
    let mut rest: Vec<&String> = all.keys().filter(|k| !order.contains(k)).collect();
    rest.sort();
    order.extend(rest.into_iter().cloned());
    let finite =
        |m: &LoroMapValue, k: &str, d: f64| float(m, k).filter(|v| v.is_finite()).unwrap_or(d);
    let mut out = Vec::new();
    for id in order {
        let Some(LoroValue::Map(m)) = all.get(&id) else {
            continue;
        };
        let mut c = CueMix::new(id, string(m, "name").unwrap_or_default());
        c.gain_db = finite(m, "gain_db", 0.0);
        c.mute = boolean(m, "mute").unwrap_or(false);
        c.follow_main = boolean(m, "follow_main").unwrap_or(false);
        c.click_db = boolean(m, "click")
            .unwrap_or(true)
            .then(|| finite(m, "click_db", 0.0));
        c.talkback = boolean(m, "talkback").unwrap_or(true);
        if let Some(LoroValue::Map(levels)) = m.get("levels") {
            // In track order, so they read back the same every time.
            for t in tracks {
                let Some(LoroValue::Map(l)) = levels.get(&t.id) else {
                    continue;
                };
                c.levels.push(CueLevel {
                    track: t.id.clone(),
                    gain_db: finite(l, "gain_db", 0.0),
                    pan: finite(l, "pan", 0.0).clamp(-1.0, 1.0),
                    mute: boolean(l, "mute").unwrap_or(false),
                });
            }
        }
        out.push(c);
    }
    out
}

fn write_comps(w: &mut Writer, m: &LoroMap, t: &Track) {
    if t.comps.is_empty() && m.get("comps").is_none() {
        return;
    }
    let comps = w.fixed(m, "comps");
    let keep: HashSet<&str> = t.comps.iter().map(|c| c.id.as_str()).collect();
    for id in keys(&comps) {
        if !keep.contains(id.as_str()) {
            w.delete(&comps, &id);
        }
    }
    for (i, c) in t.comps.iter().enumerate() {
        let cm = w.child(&comps, &c.id);
        w.put(&cm, "name", c.name.as_str());
        w.put(&cm, "n", i as i64);
        let parts: Vec<LoroValue> = (c.parts.iter())
            .map(|p| {
                LoroValue::List(vec![(p.take as i64).into(), p.start.into(), p.end.into()].into())
            })
            .collect();
        w.put(&cm, "parts", LoroValue::List(parts.into()));
    }
    w.put(m, "comp", opt_str(&t.comp));
}

/// A track's comps, in the order made. Odd parts are left out.
fn read_comps(t: &LoroMapValue) -> (Vec<Comp>, Option<String>) {
    let Some(LoroValue::Map(comps)) = t.get("comps") else {
        return (Vec::new(), None);
    };
    let mut out: Vec<(i64, Comp)> = Vec::new();
    for (id, c) in comps.iter() {
        let LoroValue::Map(c) = c else { continue };
        let mut parts: Vec<CompPart> = match c.get("parts") {
            Some(LoroValue::List(l)) => (l.iter())
                .filter_map(|p| match p {
                    LoroValue::List(p) => match &p[..] {
                        [
                            LoroValue::I64(take),
                            LoroValue::I64(start),
                            LoroValue::I64(end),
                        ] if *take >= 0 && *take <= u32::MAX as i64 && start < end => {
                            Some(CompPart {
                                take: *take as u32,
                                start: *start,
                                end: *end,
                            })
                        }
                        _ => None,
                    },
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        parts.sort_by_key(|p| p.start);
        let n = int(c, "n").unwrap_or(i64::MAX);
        out.push((
            n,
            Comp {
                id: id.to_string(),
                name: string(c, "name").unwrap_or_default(),
                parts,
            },
        ));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.id.cmp(&b.1.id)));
    let out: Vec<Comp> = out.into_iter().map(|(_, c)| c).collect();
    let comp = string(t, "comp").filter(|id| out.iter().any(|c| &c.id == id));
    (out, comp)
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
        note_clips: Vec::new(),
        instrument: None,
        effects: Vec::new(),
        frozen: t.get("frozen").and_then(read_frozen),
        freeze: boolean(t, "freeze").unwrap_or(false),
        comps: Vec::new(),
        comp: None,
        kind: TrackKind::from_name(&string(t, "kind").unwrap_or_default()),
        parent: string(t, "parent").filter(|s| !s.is_empty()),
        output: string(t, "output").filter(|s| !s.is_empty()),
        sends: Vec::new(),
        group: string(t, "group").filter(|s| !s.is_empty()),
    };
    (track.comps, track.comp) = read_comps(t);
    if let Some(LoroValue::Map(sends)) = t.get("sends") {
        for (to, sm) in sends.iter() {
            let LoroValue::Map(sm) = sm else { continue };
            track.sends.push(Send {
                to: to.to_string(),
                gain_db: float(sm, "gain_db")
                    .filter(|v| v.is_finite())
                    .unwrap_or(0.0),
                pre_fader: boolean(sm, "pre_fader").unwrap_or(false),
                mute: boolean(sm, "mute").unwrap_or(false),
            });
        }
    }
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
                comp: boolean(c, "comp").unwrap_or(false),
                seq: int(c, "seq").unwrap_or(0),
                gain_db: float(c, "gain_db").filter(|g| g.is_finite()).unwrap_or(0.0),
                fade_in: read_fade(c, "fade_in"),
                fade_out: read_fade(c, "fade_out"),
                envelope: read_envelope(c),
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
            track.effects.push(read_effect(eid, e));
        }
    }
    if let Some(LoroValue::Map(im)) = t.get("instrument") {
        let id = string(im, "id").unwrap_or_else(|| "instrument".to_string());
        track.instrument = Some(read_effect(id, im));
    }
    track.note_clips = read_note_clips(t);
    track
}

/// An effect (or instrument) from its map.
fn read_effect(id: String, e: &LoroMapValue) -> Effect {
    let state = binary(e, "component").map(|component| EffectState {
        component,
        controller: binary(e, "controller"),
    });
    Effect {
        id,
        name: string(e, "name").unwrap_or_default(),
        class: binary(e, "class").unwrap_or_default(),
        bundle: string(e, "bundle").unwrap_or_default(),
        bypass: boolean(e, "bypass").unwrap_or(false),
        state,
        builtin: string(e, "builtin").unwrap_or_default(),
        response: string(e, "response").unwrap_or_default(),
        sidechain: string(e, "sidechain").filter(|s| !s.is_empty()),
        container: string(e, "container").filter(|s| !s.is_empty()),
        branch: match e.get("branch") {
            Some(LoroValue::I64(b)) => u32::try_from(*b).unwrap_or(0),
            _ => 0,
        },
        outputs: match e.get("outputs") {
            Some(LoroValue::List(l)) => (l.iter())
                .filter_map(|v| match v {
                    LoroValue::I64(o) => u32::try_from(*o).ok(),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        },
        params: match e.get("params") {
            Some(LoroValue::Map(pm)) => {
                let mut v: Vec<(String, f64)> = (pm.keys())
                    .filter_map(|k| Some((k.to_string(), float(pm, k)?)))
                    .collect();
                v.sort_by(|a, b| a.0.cmp(&b.0));
                v
            }
            _ => Vec::new(),
        },
    }
}

fn read_fade(c: &LoroMapValue, k: &str) -> Fade {
    Fade {
        len: int(c, k).unwrap_or(0).max(0),
        curve: Curve::from_name(&string(c, &format!("{k}_curve")).unwrap_or_default()),
    }
}

/// A clip's envelope points, in order; anything odd is left out.
fn read_envelope(c: &LoroMapValue) -> Vec<GainPoint> {
    let Some(LoroValue::List(l)) = c.get("envelope") else {
        return Vec::new();
    };
    let mut points: Vec<GainPoint> = (l.iter())
        .filter_map(|p| match p {
            LoroValue::List(p) => match &p[..] {
                [LoroValue::I64(at), LoroValue::Double(db)] if *at >= 0 && db.is_finite() => {
                    Some(GainPoint { at: *at, db: *db })
                }
                _ => None,
            },
            _ => None,
        })
        .collect();
    points.sort_by_key(|p| p.at);
    points
}

fn frozen_value(f: &Frozen) -> LoroValue {
    let clips: Vec<LoroValue> = (f.clips.iter())
        .map(|c| {
            let mut v: Vec<LoroValue> = vec![
                c.asset.as_str().into(),
                c.start.into(),
                c.offset.into(),
                c.len.into(),
            ];
            // Only for a clip with fades or gain, so older versions
            // still read the rest.
            if !c.sound.is_empty() {
                v.push(c.sound.as_str().into());
            }
            LoroValue::List(v.into())
        })
        .collect();
    let chain: Vec<LoroValue> = (f.chain.iter())
        .map(|(id, bypass)| LoroValue::List(vec![id.as_str().into(), (*bypass).into()].into()))
        .collect();
    let m: HashMap<String, LoroValue> = HashMap::from([
        ("asset".to_string(), f.asset.as_str().into()),
        ("rate".to_string(), (f.rate as i64).into()),
        ("start".to_string(), f.start.into()),
        ("len".to_string(), f.len.into()),
        ("print".to_string(), f.print.as_str().into()),
        ("clips".to_string(), LoroValue::List(clips.into())),
        ("chain".to_string(), LoroValue::List(chain.into())),
    ]);
    LoroValue::Map(m.into())
}

/// Frozen audio as the document holds it. It may have come from anyone,
/// so anything odd (an asset name that could point elsewhere, a
/// negative length) means there is none.
fn read_frozen(v: &LoroValue) -> Option<Frozen> {
    let LoroValue::Map(m) = v else {
        return None;
    };
    let asset = string(m, "asset").filter(|a| is_asset_name(a))?;
    let rate = int(m, "rate").filter(|r| (1..=1_000_000).contains(r))? as u32;
    let start = int(m, "start").filter(|v| *v >= 0)?;
    let len = int(m, "len").filter(|v| *v > 0)?;
    let list = |k: &str| match m.get(k) {
        Some(LoroValue::List(l)) => Some(l.iter().cloned().collect::<Vec<_>>()),
        _ => None,
    };
    let mut clips = Vec::new();
    for c in list("clips")? {
        let LoroValue::List(c) = c else { return None };
        match &c[..] {
            [
                LoroValue::String(asset),
                LoroValue::I64(start),
                LoroValue::I64(offset),
                LoroValue::I64(len),
                sound @ ..,
            ] => clips.push(FrozenClip {
                asset: asset.to_string(),
                start: *start,
                offset: *offset,
                len: *len,
                sound: match sound {
                    [] => String::new(),
                    [LoroValue::String(s)] => s.to_string(),
                    _ => return None,
                },
            }),
            _ => return None,
        }
    }
    let mut chain = Vec::new();
    for e in list("chain")? {
        let LoroValue::List(e) = e else { return None };
        match &e[..] {
            [LoroValue::String(id), LoroValue::Bool(b)] => chain.push((id.to_string(), *b)),
            _ => return None,
        }
    }
    Some(Frozen {
        asset,
        rate,
        start,
        len,
        print: string(m, "print").unwrap_or_default(),
        clips,
        chain,
    })
}

/// Writes values only where they differ, noting whether any did.
#[derive(Default)]
pub(crate) struct Writer {
    pub(crate) changed: bool,
}

impl Writer {
    /// True if it wrote.
    pub(crate) fn put(&mut self, m: &LoroMap, key: &str, v: impl Into<LoroValue>) -> bool {
        let v = v.into();
        let now = m.get(key).and_then(|x| x.into_value().ok());
        if now.as_ref() != Some(&v) {
            let _ = m.insert(key, v);
            self.changed = true;
            return true;
        }
        false
    }

    pub(crate) fn delete(&mut self, m: &LoroMap, key: &str) {
        let _ = m.delete(key);
        self.changed = true;
    }

    /// The map at `key`, made if missing.
    pub(crate) fn child(&mut self, m: &LoroMap, key: &str) -> LoroMap {
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
    pub(crate) fn fixed(&mut self, m: &LoroMap, key: &str) -> LoroMap {
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

pub(crate) fn keys(m: &LoroMap) -> Vec<String> {
    m.keys().map(|k| k.to_string()).collect()
}

fn str_at(list: &LoroMovableList, i: usize) -> Option<String> {
    match list.get(i)?.into_value().ok()? {
        LoroValue::String(s) => Some(s.to_string()),
        _ => None,
    }
}

pub(crate) fn int(m: &LoroMapValue, k: &str) -> Option<i64> {
    match m.get(k) {
        Some(LoroValue::I64(v)) => Some(*v),
        Some(LoroValue::Double(v)) => Some(*v as i64),
        _ => None,
    }
}

pub(crate) fn float(m: &LoroMapValue, k: &str) -> Option<f64> {
    match m.get(k) {
        Some(LoroValue::Double(v)) => Some(*v),
        Some(LoroValue::I64(v)) => Some(*v as f64),
        _ => None,
    }
}

pub(crate) fn boolean(m: &LoroMapValue, k: &str) -> Option<bool> {
    match m.get(k) {
        Some(LoroValue::Bool(v)) => Some(*v),
        _ => None,
    }
}

pub(crate) fn string(m: &LoroMapValue, k: &str) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Articulation, Note, NoteClip, PPQ, Technique};
    use crate::{ControlGroup, Send, Track, TrackKind};

    #[test]
    fn routing_is_kept_and_routes_to_missing_tracks_go_to_the_master() {
        let mut p = Project::default();
        let mut guitar = Track::new("g".into(), "Guitar".into());
        guitar.parent = Some("f".into());
        guitar.group = Some("vca".into());
        guitar.sends = vec![
            Send {
                to: "r".into(),
                gain_db: -6.0,
                pre_fader: true,
                mute: false,
            },
            Send::to("gone".into()),
        ];
        guitar.effects.push(Effect {
            id: "e1".into(),
            builtin: "compressor".into(),
            sidechain: Some("b".into()),
            ..Effect::default()
        });
        guitar.effects.push(Effect {
            id: "e2".into(),
            class: vec![7; 16],
            sidechain: Some("b".into()),
            outputs: vec![1, 2],
            ..Effect::default()
        });
        let mut bass = Track::new("b".into(), "Bass".into());
        bass.output = Some("nowhere".into());
        let mut folder = Track::new("f".into(), "Band".into());
        folder.kind = TrackKind::Folder;
        let mut ret = Track::new("r".into(), "Reverb".into());
        ret.kind = TrackKind::Bus;
        p.tracks = vec![folder, guitar, bass, ret];
        p.control_groups = vec![ControlGroup::new("vca".into(), "Rhythm".into())];
        let mut doc = ProjectDoc::new(1);
        doc.set(&p);
        let back = doc.get();
        assert_eq!(back.control_groups, p.control_groups);
        assert_eq!(back.tracks[0].kind, TrackKind::Folder);
        assert_eq!(back.tracks[3].kind, TrackKind::Bus);
        let g = &back.tracks[1];
        assert_eq!(g.parent.as_deref(), Some("f"));
        assert_eq!(g.group.as_deref(), Some("vca"));
        // The send to a track that isn't there is left out.
        assert_eq!(g.sends, p.tracks[1].sends[..1]);
        assert_eq!(g.effects[0].sidechain.as_deref(), Some("b"));
        assert_eq!(g.effects[1].outputs, vec![1, 2]);
        assert!(g.effects[0].outputs.is_empty());
        assert_eq!(back.tracks[2].output, None, "to the master");

        // Taking things off writes them as gone, and reads back plain.
        p.tracks[1].sends.clear();
        p.tracks[1].group = None;
        p.tracks[1].effects[0].sidechain = None;
        p.tracks[1].effects[1].outputs.clear();
        p.control_groups.clear();
        doc.set(&p);
        let back = doc.get();
        assert!(back.tracks[1].sends.is_empty());
        assert_eq!(back.tracks[1].group, None);
        assert_eq!(back.tracks[1].effects[0].sidechain, None);
        assert!(back.tracks[1].effects[1].outputs.is_empty());
        assert!(back.control_groups.is_empty());
    }

    #[test]
    fn timecode_and_hit_points_are_kept() {
        let mut doc = ProjectDoc::new(1);
        doc.set(&Project::default());
        // Not written until it is set up.
        assert!(doc.song().get("tc_rate").is_none());
        let mut p = Project {
            timecode: TimecodeSetup {
                rate: "29.97df".into(),
                start: "01:00:00;00".into(),
                offset: 96_000,
                shown: true,
            },
            hit_points: vec![
                HitPoint {
                    id: "h2".into(),
                    at: 480_000,
                    name: "Cut to exterior".into(),
                },
                HitPoint {
                    id: "h1".into(),
                    at: 200_200,
                    name: "Door slams".into(),
                },
            ],
            ..Project::default()
        };
        doc.set(&p);
        let back = doc.get();
        assert_eq!(back.timecode, p.timecode);
        // In time order.
        assert_eq!(back.hit_points[0], p.hit_points[1]);
        assert_eq!(back.hit_points[1], p.hit_points[0]);
        p.hit_points.remove(0);
        p.timecode = TimecodeSetup::default();
        doc.set(&p);
        let back = doc.get();
        assert_eq!(back.hit_points.len(), 1);
        assert_eq!(back.hit_points[0].name, "Door slams");
        assert_eq!(back.timecode, TimecodeSetup::default());
    }

    #[test]
    fn headphone_mixes_are_kept_without_tracks_that_are_gone() {
        let mut p = Project {
            tracks: vec![
                Track::new("g".into(), "Guitar".into()),
                Track::new("v".into(), "Voice".into()),
            ],
            ..Project::default()
        };
        let mut student = CueMix::new("c1".into(), "Student".into());
        student.gain_db = -3.0;
        student.click_db = None;
        student.talkback = false;
        student.set_level(CueLevel {
            pan: -0.5,
            mute: true,
            ..CueLevel::new("v".into(), -6.0)
        });
        student.set_level(CueLevel::new("gone".into(), 0.0));
        let mut teacher = CueMix::new("c2".into(), "Teacher".into());
        teacher.follow_main = true;
        p.cue_mixes = vec![student.clone(), teacher.clone()];
        let mut doc = ProjectDoc::new(1);
        doc.set(&p);
        let back = doc.get();
        student.levels.retain(|l| l.track != "gone");
        assert_eq!(back.cue_mixes, [student.clone(), teacher.clone()]);
        // Not listed: an audio track is in at 0 dB.
        assert_eq!(back.cue_mixes[0].level(&p.tracks[0]).gain_db, 0.0);

        p.cue_mixes = vec![teacher.clone(), student];
        doc.set(&p);
        assert_eq!(doc.get().cue_mixes[0].name, "Teacher", "reordered");
        p.cue_mixes.clear();
        doc.set(&p);
        assert!(doc.get().cue_mixes.is_empty());
    }

    #[test]
    fn plain_tracks_add_no_routing_entries() {
        let p = Project {
            tracks: vec![Track::new("t1".into(), "Guitar".into())],
            ..Project::default()
        };
        let mut doc = ProjectDoc::new(1);
        doc.set(&p);
        let LoroValue::Map(t) = doc.tracks().get_deep_value() else {
            panic!()
        };
        let Some(LoroValue::Map(t)) = t.get("t1") else {
            panic!()
        };
        for k in ["kind", "parent", "output", "group", "sends"] {
            assert!(t.get(k).is_none(), "{k}");
        }
        let LoroValue::Map(root) = doc.doc.get_deep_value() else {
            panic!()
        };
        assert!(root.get("groups").is_none());
        assert!(root.get("cue_mixes").is_none());
    }

    #[test]
    fn comps_keep_their_order_parts_and_which_plays() {
        let mut t = Track::new("t1".into(), "Guitar".into());
        t.clips.push(Clip {
            id: "c1".into(),
            asset: "a.wav".into(),
            len: 100,
            take: Some(0),
            comp: true,
            ..Default::default()
        });
        let part = |take, start, end| CompPart { take, start, end };
        for (id, name) in [("zz", "Comp 1"), ("aa", "Verse best")] {
            t.comps.push(Comp {
                id: id.into(),
                name: name.into(),
                parts: vec![part(0, 0, 50), part(1, 50, 100)],
            });
        }
        t.comp = Some("aa".into());
        let p = Project {
            tracks: vec![t],
            ..Project::default()
        };
        let mut doc = ProjectDoc::new(1);
        doc.set(&p);
        assert_eq!(doc.get(), p);
        // A track never comped gains no entries.
        let mut plain = Project::default();
        plain.tracks.push(Track::new("t2".into(), "Bass".into()));
        let mut doc = ProjectDoc::new(2);
        doc.set(&plain);
        let LoroValue::Map(ts) = doc.tracks().get_deep_value() else {
            panic!()
        };
        let Some(LoroValue::Map(t)) = ts.get("t2") else {
            panic!()
        };
        assert!(t.get("comps").is_none() && t.get("comp").is_none());
    }

    #[test]
    fn clips_at_their_plain_level_add_no_entries() {
        // So a project from before fades saves as it did.
        let mut t = Track::new("t1".into(), "Guitar".into());
        t.clips.push(Clip {
            id: "c1".into(),
            asset: "a.wav".into(),
            len: 100,
            ..Default::default()
        });
        let mut p = Project {
            tracks: vec![t],
            ..Project::default()
        };
        let mut doc = ProjectDoc::new(1);
        doc.set(&p);
        let keys = |doc: &ProjectDoc| {
            let LoroValue::Map(t) = doc.tracks().get_deep_value() else {
                panic!()
            };
            let Some(LoroValue::Map(t)) = t.get("t1") else {
                panic!()
            };
            let Some(LoroValue::Map(c)) = t.get("clips") else {
                panic!()
            };
            let Some(LoroValue::Map(c)) = c.get("c1") else {
                panic!()
            };
            let mut k: Vec<String> = c.keys().cloned().collect();
            k.sort();
            k
        };
        assert!(
            !keys(&doc)
                .iter()
                .any(|k| k.contains("fade") || k == "gain_db")
        );
        p.tracks[0].clips[0].fade_out.len = 10;
        doc.set(&p);
        assert!(keys(&doc).contains(&"fade_out_curve".to_string()));
        // Taken off again, the entry stays, at none.
        p.tracks[0].clips[0].fade_out.len = 0;
        doc.set(&p);
        assert_eq!(doc.get().tracks[0].clips[0].fade_out.len, 0);
    }

    #[test]
    fn edit_groups_round_trip_and_merge_tracks_added_at_once() {
        let group = EditGroup {
            id: "g1".into(),
            name: "Drums".into(),
            tracks: vec!["kick".into(), "snare".into()],
            active: true,
            created: 5,
        };
        let p = Project {
            edit_groups: vec![group],
            ..Project::default()
        };
        let mut a = ProjectDoc::new(1);
        a.set(&p);
        assert_eq!(a.get().edit_groups, p.edit_groups);

        let mut b = ProjectDoc::from_bytes(&a.to_bytes(), 2).unwrap();
        let mut pa = a.get();
        pa.edit_groups[0].tracks.push("tom".into());
        a.set(&pa);
        let mut pb = b.get();
        pb.edit_groups[0].tracks.retain(|t| t != "snare");
        pb.edit_groups[0].active = false;
        b.set(&pb);
        let (va, vb) = (a.version(), b.version());
        let to_b = a.updates_since(&vb).unwrap();
        let to_a = b.updates_since(&va).unwrap();
        a.merge(&to_a).unwrap();
        b.merge(&to_b).unwrap();
        let g = &a.get().edit_groups[0];
        assert_eq!(g.tracks, ["kick", "tom"]);
        assert!(!g.active);
        assert_eq!(a.get(), b.get());

        let mut gone = a.get();
        gone.edit_groups.clear();
        a.set(&gone);
        assert!(a.get().edit_groups.is_empty());
    }

    #[test]
    fn effects_in_containers_round_trip_and_merge_chains_filled_at_once() {
        let mut p = Project::default();
        let mut t = Track::new("g".into(), "Guitar".into());
        t.effects = vec![
            Effect {
                id: "split".into(),
                builtin: "band-split".into(),
                params: vec![("bands".into(), 0.0)],
                ..Effect::default()
            },
            Effect {
                id: "low".into(),
                builtin: "compressor".into(),
                container: Some("split".into()),
                ..Effect::default()
            },
        ];
        p.tracks = vec![t];
        let mut a = ProjectDoc::new(1);
        a.set(&p);
        assert_eq!(a.get().tracks[0].effects, p.tracks[0].effects);

        // Two people each put an effect in a different band at once.
        let mut b = ProjectDoc::from_bytes(&a.to_bytes(), 2).unwrap();
        let mut pa = a.get();
        pa.tracks[0].effects.push(Effect {
            id: "high".into(),
            builtin: "delay".into(),
            container: Some("split".into()),
            branch: 1,
            ..Effect::default()
        });
        a.set(&pa);
        let mut pb = b.get();
        pb.tracks[0].effects.push(Effect {
            id: "low2".into(),
            builtin: "eq".into(),
            container: Some("split".into()),
            ..Effect::default()
        });
        b.set(&pb);
        let (va, vb) = (a.version(), b.version());
        let to_b = a.updates_since(&vb).unwrap();
        let to_a = b.updates_since(&va).unwrap();
        a.merge(&to_a).unwrap();
        b.merge(&to_b).unwrap();
        assert_eq!(a.get(), b.get());
        let fx = &a.get().tracks[0].effects;
        assert_eq!(fx.len(), 4);
        let high = fx.iter().find(|e| e.id == "high").unwrap();
        assert_eq!((high.container.as_deref(), high.branch), (Some("split"), 1));

        // Taking an effect out of its container puts it back on the
        // track's own chain.
        let mut out = a.get();
        out.tracks[0].effects[1].container = None;
        a.set(&out);
        let low = &a.get().tracks[0].effects[1];
        assert_eq!((low.container.as_deref(), low.branch), (None, 0));
    }

    #[test]
    fn note_clips_and_the_instrument_round_trip() {
        let mut p = Project::default();
        let mut t = Track::new("k".into(), "Keys".into());
        t.kind = TrackKind::Instrument;
        let mut clip = NoteClip::new("nc1".into(), "Riff".into(), PPQ * 4, PPQ * 8);
        clip.offset = PPQ;
        clip.take = Some(2);
        let mut a = Note::new(PPQ, PPQ / 2, 64, 90);
        a.id = "n1".into();
        a.string = Some(3);
        a.fret = Some(9);
        a.bend = 1.0;
        a.techniques = vec![Technique::Vibrato, Technique::PalmMute];
        a.channel = 2;
        let mut b = Note::new(0, PPQ, 36, 127);
        b.id = "n2".into();
        b.articulation = Some(Articulation::Flam);
        b.chance = 0.5;
        clip.notes = vec![a.clone(), b.clone()];
        t.note_clips.push(clip);
        t.instrument = Some(Effect {
            id: "inst".into(),
            name: "Simple synth".into(),
            builtin: "synth".into(),
            params: vec![("wave".into(), 1.0)],
            ..Effect::default()
        });
        p.tracks.push(t);
        let mut doc = ProjectDoc::new(1);
        assert!(doc.set(&p));
        let back = ProjectDoc::from_bytes(&doc.to_bytes(), 2).unwrap().get();
        assert_eq!(back.tracks[0].kind, TrackKind::Instrument);
        let clip = &back.tracks[0].note_clips[0];
        assert_eq!(
            (clip.start, clip.len, clip.offset, clip.take),
            (PPQ * 4, PPQ * 8, PPQ, Some(2))
        );
        // Notes come back in time order.
        assert_eq!(clip.notes, vec![b, a]);
        assert_eq!(back.tracks[0].instrument, p.tracks[0].instrument);
        // Writing the same again changes nothing.
        assert!(!doc.set(&back));
        // Deleting one note leaves the other.
        let mut q = back.clone();
        q.tracks[0].note_clips[0].notes.remove(0);
        q.tracks[0].instrument = None;
        assert!(doc.set(&q));
        let back = doc.get();
        assert_eq!(back.tracks[0].note_clips[0].notes.len(), 1);
        assert_eq!(back.tracks[0].note_clips[0].notes[0].id, "n1");
        assert_eq!(back.tracks[0].instrument, None);
    }

    #[test]
    fn plain_tracks_and_notes_add_no_note_entries() {
        let mut p = Project::default();
        let mut t = Track::new("k".into(), "Keys".into());
        let mut clip = NoteClip::new("nc1".into(), "Riff".into(), 0, PPQ);
        let mut n = Note::new(0, PPQ, 60, 100);
        n.id = "n1".into();
        clip.notes.push(n);
        t.note_clips.push(clip);
        p.tracks.push(t);
        p.tracks.push(Track::new("g".into(), "Guitar".into()));
        let mut doc = ProjectDoc::new(1);
        doc.set(&p);
        let tracks = doc.doc.get_map("tracks").get_deep_value();
        let LoroValue::Map(tracks) = tracks else {
            panic!()
        };
        let LoroValue::Map(g) = tracks.get("g").unwrap() else {
            panic!()
        };
        assert!(g.get("note_clips").is_none());
        assert!(g.get("instrument").is_none());
        let LoroValue::Map(k) = tracks.get("k").unwrap() else {
            panic!()
        };
        let LoroValue::Map(clips) = k.get("note_clips").unwrap() else {
            panic!()
        };
        let LoroValue::Map(c) = clips.get("nc1").unwrap() else {
            panic!()
        };
        let LoroValue::Map(notes) = c.get("notes").unwrap() else {
            panic!()
        };
        let LoroValue::Map(n) = notes.get("n1").unwrap() else {
            panic!()
        };
        let mut keys: Vec<&str> = n.keys().map(|k| k.as_str()).collect();
        keys.sort();
        assert_eq!(keys, ["len", "pitch", "start", "velocity"]);
    }

    #[test]
    fn two_people_editing_one_note_clip_keep_both_notes() {
        let mut p = Project::default();
        let mut t = Track::new("k".into(), "Keys".into());
        t.note_clips
            .push(NoteClip::new("nc1".into(), "Riff".into(), 0, PPQ * 4));
        p.tracks.push(t);
        let mut a = ProjectDoc::new(1);
        a.set(&p);
        let mut b = ProjectDoc::from_bytes(&a.to_bytes(), 2).unwrap();
        let mut pa = a.get();
        let mut na = Note::new(0, PPQ, 60, 100);
        na.id = "na".into();
        pa.tracks[0].note_clips[0].notes.push(na);
        a.set(&pa);
        let mut pb = b.get();
        let mut nb = Note::new(PPQ, PPQ, 64, 100);
        nb.id = "nb".into();
        pb.tracks[0].note_clips[0].notes.push(nb);
        b.set(&pb);
        a.merge(&b.to_bytes()).unwrap();
        let ids: Vec<String> = (a.get().tracks[0].note_clips[0].notes.iter())
            .map(|n| n.id.clone())
            .collect();
        assert_eq!(ids, ["na", "nb"]);
    }
}
