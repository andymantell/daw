// SPDX-License-Identifier: GPL-3.0-or-later
//! The project document on yrs. yrs has no movable list (a move would be a
//! delete plus insert, which duplicates items under concurrent moves), so
//! track order uses a fractional position stored on each track.

use crate::{Clip, Project, ProjectDoc, Track, order_by_position, position_for};
use yrs::undo::Options;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{
    Any, Doc, Map, MapPrelim, MapRef, Out, ReadTxn, StateVector, Transact, TransactionMut,
    UndoManager, Update,
};

pub struct YrsProject {
    doc: Doc,
    tracks: MapRef,
    undo: UndoManager,
}

/// Origin for changes received from other peers. The undo manager only
/// tracks transactions without an origin, i.e. local edits.
const REMOTE: &str = "remote";

fn sub_map<T: ReadTxn>(txn: &T, m: &MapRef, k: &str) -> Option<MapRef> {
    match m.get(txn, k)? {
        Out::YMap(m) => Some(m),
        _ => None,
    }
}

fn num<T: ReadTxn>(txn: &T, m: &MapRef, k: &str) -> f64 {
    match m.get(txn, k) {
        Some(Out::Any(Any::Number(n))) => n.as_f64().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn int<T: ReadTxn>(txn: &T, m: &MapRef, k: &str) -> i64 {
    match m.get(txn, k) {
        Some(Out::Any(Any::Number(n))) => n.as_i64().unwrap_or(0),
        _ => 0,
    }
}

impl YrsProject {
    fn edit(&mut self, f: impl FnOnce(&mut TransactionMut, &MapRef)) {
        {
            let mut txn = self.doc.transact_mut();
            f(&mut txn, &self.tracks);
        }
        // Close the undo step so each edit undoes on its own.
        self.undo.reset();
    }

    /// Positions of all tracks except `skip`, sorted.
    fn sorted_positions<T: ReadTxn>(&self, txn: &T, skip: Option<&str>) -> Vec<f64> {
        let mut v: Vec<f64> = self
            .tracks
            .iter(txn)
            .filter(|(id, _)| Some(*id) != skip)
            .filter_map(|(_, t)| match t {
                Out::YMap(t) => Some(num(txn, &t, "pos")),
                _ => None,
            })
            .collect();
        v.sort_by(f64::total_cmp);
        v
    }
}

impl ProjectDoc for YrsProject {
    fn new(peer: u64) -> Self {
        let doc = Doc::with_client_id(peer);
        let tracks = doc.get_or_insert_map("tracks");
        let mut undo = UndoManager::with_options(Options {
            capture_timeout_millis: 0,
            ..Default::default()
        });
        undo.expand_scope(&doc, &tracks);
        Self { doc, tracks, undo }
    }

    fn add_track(&mut self, id: &str, name: &str) {
        let pos = {
            let txn = self.doc.transact();
            self.sorted_positions(&txn, None)
                .last()
                .map_or(1.0, |p| p + 1.0)
        };
        self.edit(|txn, tracks| {
            let t = tracks.insert(txn, id, MapPrelim::default());
            t.insert(txn, "name", name);
            t.insert(txn, "gain", 1.0);
            t.insert(txn, "pos", pos);
            t.insert(txn, "clips", MapPrelim::default());
        });
    }

    fn rename_track(&mut self, id: &str, name: &str) {
        self.edit(|txn, tracks| {
            if let Some(t) = sub_map(txn, tracks, id) {
                t.insert(txn, "name", name);
            }
        });
    }

    fn set_gain(&mut self, id: &str, gain: f64) {
        self.edit(|txn, tracks| {
            if let Some(t) = sub_map(txn, tracks, id) {
                t.insert(txn, "gain", gain);
            }
        });
    }

    fn delete_track(&mut self, id: &str) {
        self.edit(|txn, tracks| {
            tracks.remove(txn, id);
        });
    }

    fn move_track(&mut self, id: &str, to: usize) {
        let pos = {
            let txn = self.doc.transact();
            position_for(&self.sorted_positions(&txn, Some(id)), to)
        };
        self.edit(|txn, tracks| {
            if let Some(t) = sub_map(txn, tracks, id) {
                t.insert(txn, "pos", pos);
            }
        });
    }

    fn add_clip(&mut self, track: &str, clip: &str, start: i64, len: i64) {
        self.edit(|txn, tracks| {
            if let Some(clips) = sub_map(txn, tracks, track).and_then(|t| sub_map(txn, &t, "clips"))
            {
                let c = clips.insert(txn, clip, MapPrelim::default());
                c.insert(txn, "start", start);
                c.insert(txn, "len", len);
            }
        });
    }

    fn move_clip(&mut self, track: &str, clip: &str, start: i64) {
        self.edit(|txn, tracks| {
            let c = sub_map(txn, tracks, track)
                .and_then(|t| sub_map(txn, &t, "clips"))
                .and_then(|clips| sub_map(txn, &clips, clip));
            if let Some(c) = c {
                c.insert(txn, "start", start);
            }
        });
    }

    fn undo(&mut self) -> bool {
        self.undo.undo_blocking()
    }

    fn project(&self) -> Project {
        let txn = self.doc.transact();
        let mut p = Project::default();
        let mut positions = Vec::new();
        for (id, t) in self.tracks.iter(&txn) {
            let Out::YMap(t) = t else { continue };
            let mut track = Track {
                name: match t.get(&txn, "name") {
                    Some(Out::Any(Any::String(s))) => s.to_string(),
                    _ => String::new(),
                },
                gain: num(&txn, &t, "gain"),
                ..Default::default()
            };
            if let Some(clips) = sub_map(&txn, &t, "clips") {
                for (cid, c) in clips.iter(&txn) {
                    let Out::YMap(c) = c else { continue };
                    track.clips.insert(
                        cid.to_string(),
                        Clip {
                            start: int(&txn, &c, "start"),
                            len: int(&txn, &c, "len"),
                        },
                    );
                }
            }
            positions.push((num(&txn, &t, "pos"), id.to_string()));
            p.tracks.insert(id.to_string(), track);
        }
        p.order = order_by_position(positions);
        p
    }

    fn version(&self) -> Vec<u8> {
        self.doc.transact().state_vector().encode_v1()
    }

    fn updates_since(&self, version: &[u8]) -> Vec<u8> {
        let sv = StateVector::decode_v1(version).unwrap();
        self.doc.transact().encode_diff_v1(&sv)
    }

    fn apply(&mut self, updates: &[u8]) {
        let mut txn = self.doc.transact_mut_with(REMOTE);
        txn.apply_update(Update::decode_v1(updates).unwrap())
            .unwrap();
    }

    fn snapshot(&self) -> Vec<u8> {
        self.doc
            .transact()
            .encode_state_as_update_v1(&StateVector::default())
    }
}
