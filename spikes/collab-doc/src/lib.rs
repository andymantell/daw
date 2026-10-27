// SPDX-License-Identifier: GPL-3.0-or-later
//! Spike 0.5, first half: the project document on a CRDT.
//!
//! The same small project model (tracks with clips, in a user-chosen order)
//! is implemented on two candidate libraries behind one trait, so the
//! same tests check both: concurrent edits, offline merge, per-user undo
//! and document size. The live network transport is the other half of the
//! spike and is not here.

pub mod loro_doc;
pub mod yrs_doc;

use std::collections::BTreeMap;

/// The canonical, library-independent view of a document. Two documents
/// have converged when their `Project`s are equal.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Project {
    /// Track ids in display order.
    pub order: Vec<String>,
    pub tracks: BTreeMap<String, Track>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub name: String,
    pub gain: f64,
    pub clips: BTreeMap<String, Clip>,
}

/// Clip positions are integer ticks (decision 0005).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Clip {
    pub start: i64,
    pub len: i64,
}

/// One replica of the project document, owned by one peer.
///
/// Ids are chosen by callers and must be unique across peers (the real
/// schema would derive them from the peer id).
pub trait ProjectDoc {
    fn new(peer: u64) -> Self;

    fn add_track(&mut self, id: &str, name: &str);
    fn rename_track(&mut self, id: &str, name: &str);
    fn set_gain(&mut self, id: &str, gain: f64);
    fn delete_track(&mut self, id: &str);
    /// Moves a track to `to` in the display order (clamped).
    fn move_track(&mut self, id: &str, to: usize);
    fn add_clip(&mut self, track: &str, clip: &str, start: i64, len: i64);
    fn move_clip(&mut self, track: &str, clip: &str, start: i64);

    /// Undoes this peer's most recent edit, leaving others' edits alone.
    fn undo(&mut self) -> bool;

    fn project(&self) -> Project;

    /// An encoding of what this replica has seen.
    fn version(&self) -> Vec<u8>;
    /// Changes this replica has that a replica at `version` lacks.
    fn updates_since(&self, version: &[u8]) -> Vec<u8>;
    fn apply(&mut self, updates: &[u8]);
    /// Full state plus history, as it would be saved to disk.
    fn snapshot(&self) -> Vec<u8>;
}

/// Exchanges missing changes both ways.
pub fn sync<D: ProjectDoc>(a: &mut D, b: &mut D) {
    let to_b = a.updates_since(&b.version());
    let to_a = b.updates_since(&a.version());
    b.apply(&to_b);
    a.apply(&to_a);
}

/// Builds the display order from per-track fractional positions, for
/// libraries without a movable list. Ties break on id so every replica
/// agrees.
pub(crate) fn order_by_position(mut positions: Vec<(f64, String)>) -> Vec<String> {
    positions.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    positions.into_iter().map(|(_, id)| id).collect()
}

/// A position that sorts at index `to` of `sorted` (excluding the track
/// being moved).
pub(crate) fn position_for(sorted: &[f64], to: usize) -> f64 {
    let to = to.min(sorted.len());
    match (to.checked_sub(1).map(|i| sorted[i]), sorted.get(to)) {
        (None, None) => 1.0,
        (None, Some(&next)) => next - 1.0,
        (Some(prev), None) => prev + 1.0,
        (Some(prev), Some(&next)) => (prev + next) / 2.0,
    }
}
