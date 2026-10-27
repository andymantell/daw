// SPDX-License-Identifier: GPL-3.0-or-later
//! The project document on Loro. Track order uses Loro's movable list.

use crate::{Clip, Project, ProjectDoc, Track};
use loro::{
    ExportMode, LoroDoc, LoroMap, LoroMovableList, LoroValue, UndoManager, ValueOrContainer,
    VersionVector,
};

pub struct LoroProject {
    doc: LoroDoc,
    undo: UndoManager,
}

impl LoroProject {
    fn tracks(&self) -> LoroMap {
        self.doc.get_map("tracks")
    }

    fn order(&self) -> LoroMovableList {
        self.doc.get_movable_list("order")
    }

    fn track(&self, id: &str) -> Option<LoroMap> {
        child_map(self.tracks().get(id))
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        let order = self.order();
        (0..order.len()).find(|&i| value_str(order.get(i)).as_deref() == Some(id))
    }

    fn commit(&self) {
        self.doc.commit();
    }
}

fn child_map(v: Option<ValueOrContainer>) -> Option<LoroMap> {
    v?.into_container().ok()?.into_map().ok()
}

fn value_str(v: Option<ValueOrContainer>) -> Option<String> {
    match v?.into_value().ok()? {
        LoroValue::String(s) => Some(s.to_string()),
        _ => None,
    }
}

fn get<'a>(m: &'a loro::LoroMapValue, k: &str) -> Option<&'a LoroValue> {
    m.get(k)
}

impl ProjectDoc for LoroProject {
    fn new(peer: u64) -> Self {
        let doc = LoroDoc::new();
        doc.set_peer_id(peer).unwrap();
        let mut undo = UndoManager::new(&doc);
        // One undo step per edit, however quickly edits follow each other.
        undo.set_merge_interval(0);
        Self { doc, undo }
    }

    fn add_track(&mut self, id: &str, name: &str) {
        let t = self.tracks().insert_container(id, LoroMap::new()).unwrap();
        t.insert("name", name).unwrap();
        t.insert("gain", 1.0).unwrap();
        t.insert_container("clips", LoroMap::new()).unwrap();
        self.order().push(id).unwrap();
        self.commit();
    }

    fn rename_track(&mut self, id: &str, name: &str) {
        if let Some(t) = self.track(id) {
            t.insert("name", name).unwrap();
            self.commit();
        }
    }

    fn set_gain(&mut self, id: &str, gain: f64) {
        if let Some(t) = self.track(id) {
            t.insert("gain", gain).unwrap();
            self.commit();
        }
    }

    fn delete_track(&mut self, id: &str) {
        self.tracks().delete(id).unwrap();
        if let Some(i) = self.index_of(id) {
            self.order().delete(i, 1).unwrap();
        }
        self.commit();
    }

    fn move_track(&mut self, id: &str, to: usize) {
        if let Some(from) = self.index_of(id) {
            let to = to.min(self.order().len() - 1);
            if from != to {
                self.order().mov(from, to).unwrap();
                self.commit();
            }
        }
    }

    fn add_clip(&mut self, track: &str, clip: &str, start: i64, len: i64) {
        if let Some(clips) = self.track(track).and_then(|t| child_map(t.get("clips"))) {
            let c = clips.insert_container(clip, LoroMap::new()).unwrap();
            c.insert("start", start).unwrap();
            c.insert("len", len).unwrap();
            self.commit();
        }
    }

    fn move_clip(&mut self, track: &str, clip: &str, start: i64) {
        let c = self
            .track(track)
            .and_then(|t| child_map(t.get("clips")))
            .and_then(|clips| child_map(clips.get(clip)));
        if let Some(c) = c {
            c.insert("start", start).unwrap();
            self.commit();
        }
    }

    fn undo(&mut self) -> bool {
        self.undo.undo().unwrap()
    }

    fn project(&self) -> Project {
        let root = self.doc.get_deep_value();
        let LoroValue::Map(root) = root else {
            unreachable!()
        };
        let mut p = Project::default();
        if let Some(LoroValue::Map(tracks)) = get(&root, "tracks") {
            for (id, t) in tracks.iter() {
                let LoroValue::Map(t) = t else { continue };
                let mut track = Track {
                    name: match get(t, "name") {
                        Some(LoroValue::String(s)) => s.to_string(),
                        _ => String::new(),
                    },
                    gain: match get(t, "gain") {
                        Some(LoroValue::Double(g)) => *g,
                        _ => 0.0,
                    },
                    ..Default::default()
                };
                if let Some(LoroValue::Map(clips)) = get(t, "clips") {
                    for (cid, c) in clips.iter() {
                        let LoroValue::Map(c) = c else { continue };
                        let int = |k| match get(c, k) {
                            Some(LoroValue::I64(v)) => *v,
                            _ => 0,
                        };
                        track.clips.insert(
                            cid.to_string(),
                            Clip {
                                start: int("start"),
                                len: int("len"),
                            },
                        );
                    }
                }
                p.tracks.insert(id.to_string(), track);
            }
        }
        if let Some(LoroValue::List(order)) = get(&root, "order") {
            for v in order.iter() {
                if let LoroValue::String(s) = v {
                    // Skip ids whose track was deleted concurrently.
                    if p.tracks.contains_key(s.as_str()) && !p.order.iter().any(|o| o == s.as_str())
                    {
                        p.order.push(s.to_string());
                    }
                }
            }
        }
        p
    }

    fn version(&self) -> Vec<u8> {
        self.doc.oplog_vv().encode()
    }

    fn updates_since(&self, version: &[u8]) -> Vec<u8> {
        let vv = VersionVector::decode(version).unwrap();
        self.doc.export(ExportMode::updates(&vv)).unwrap()
    }

    fn apply(&mut self, updates: &[u8]) {
        self.doc.import(updates).unwrap();
    }

    fn snapshot(&self) -> Vec<u8> {
        self.doc.export(ExportMode::Snapshot).unwrap()
    }
}
