// SPDX-License-Identifier: GPL-3.0-or-later
//! Projects (roadmap 1.6, decision 0004): a folder holding the project
//! document, a Loro CRDT, and the audio it uses, each file named by its
//! content.
//!
//! The app keeps its own model and hands this crate a plain [`Project`]
//! after each edit; [`ProjectDoc::set`] writes only what changed into the
//! document, so edits made elsewhere to other fields survive a merge.
//! Undo is the document's, per user.

mod doc;
mod folder;
mod identity;
mod model;
mod snapshots;

pub use doc::ProjectDoc;
pub use folder::{ASSETS, DEVICES, DOC_FILE, ProjectFolder, is_asset_name};
pub use identity::{Check, Identity, MAX_NAME, Member, Role, clean_name};
pub use model::{
    Clip, Comment, Effect, EffectState, FROZEN_TAIL_SECONDS, Frozen, FrozenClip, Marker, Practice,
    Project, TabBlock, Track, TrainerSettings,
};
pub use snapshots::{MAX_SNAPSHOT_NAME, Snapshot};

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// A new id for a track, clip, effect, marker, comment or tab block: 16 hex digits, unique
/// enough that two people adding things at once never pick the same one.
pub fn new_id() -> String {
    format!("{:016x}", random_u64())
}

/// A peer id for this run of the app.
pub fn new_peer() -> u64 {
    random_u64()
}

fn random_u64() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    // SplitMix64 over the time, process and count.
    let mut z = nanos ^ ((std::process::id() as u64) << 40) ^ n.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}
