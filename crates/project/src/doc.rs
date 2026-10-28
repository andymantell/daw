// SPDX-License-Identifier: GPL-3.0-or-later
//! The project document on Loro.
//!
//! Layout: a `song` map of settings, a `tracks` map from id to track,
//! with the track order in the movable list `order`, a `markers` map, and
//! the lesson notes as text. A track is a map of its settings, a `clips`
//! map from id to clip, an `fx` map from id to effect and the effect
//! order in the movable list `fx_order`. Every field is its own map entry,
//! so two people changing different fields of the same clip both keep
//! their change. A `practice` map holds each person's practice settings
//! under their own key, which only they write.

use std::collections::HashSet;

use loro::{
    ExportMode, LoroDoc, LoroMap, LoroMapValue, LoroMovableList, LoroText, LoroValue, UndoManager,
    UpdateOptions, VersionVector,
};

use crate::{Clip, Effect, EffectState, Marker, Practice, Project, Track, TrainerSettings};

/// Commits with this origin (plugin state written at save time) are left
/// out of undo.
const QUIET: &str = "quiet";

pub struct ProjectDoc {
    doc: LoroDoc,
    undo: UndoManager,
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
        Ok(ProjectDoc::wrap(doc))
    }

    fn wrap(doc: LoroDoc) -> ProjectDoc {
        let mut undo = UndoManager::new(&doc);
        // One step per edit, however quickly they follow each other.
        undo.set_merge_interval(0);
        undo.set_max_undo_steps(500);
        undo.add_exclude_origin_prefix(QUIET);
        ProjectDoc { doc, undo }
    }

    /// The whole document, history included, for saving.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.doc.export(ExportMode::Snapshot).unwrap_or_default()
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
        self.doc
            .import(bytes)
            .map(|_| ())
            .map_err(|e| e.to_string())
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
        p
    }
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
    fn put(&mut self, m: &LoroMap, key: &str, v: impl Into<LoroValue>) {
        let v = v.into();
        let now = m.get(key).and_then(|x| x.into_value().ok());
        if now.as_ref() != Some(&v) {
            let _ = m.insert(key, v);
            self.changed = true;
        }
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
