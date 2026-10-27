// SPDX-License-Identifier: GPL-3.0-or-later
//! The same scenarios run against both CRDT candidates.

use spike_collab_doc::loro_doc::LoroProject;
use spike_collab_doc::yrs_doc::YrsProject;
use spike_collab_doc::{Project, ProjectDoc, sync};
use std::collections::BTreeSet;

struct XorShift(u64);
impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Teacher-side starting point: three tracks with a clip each, shared with
/// the student.
fn lesson<D: ProjectDoc>() -> (D, D) {
    let mut a = D::new(1);
    let mut b = D::new(2);
    for (i, name) in ["Backing", "Riff", "Click"].iter().enumerate() {
        let id = format!("t{i}");
        a.add_track(&id, name);
        a.add_clip(&id, &format!("c{i}"), 0, 3840);
    }
    sync(&mut a, &mut b);
    assert_eq!(a.project(), b.project());
    (a, b)
}

fn assert_well_formed(p: &Project) {
    let ids: BTreeSet<_> = p.tracks.keys().cloned().collect();
    let ordered: BTreeSet<_> = p.order.iter().cloned().collect();
    assert_eq!(
        p.order.len(),
        ordered.len(),
        "duplicate in order: {:?}",
        p.order
    );
    assert_eq!(ids, ordered, "order and tracks disagree");
}

fn concurrent_edits_converge<D: ProjectDoc>() {
    let (mut a, mut b) = lesson::<D>();
    // Teacher and student edit at the same time, unaware of each other.
    a.rename_track("t0", "Backing (teacher)");
    a.move_clip("t0", "c0", 960);
    a.set_gain("t1", 0.5);
    b.rename_track("t0", "Backing (student)");
    b.add_clip("t1", "b-take1", 7680, 1920);
    b.move_track("t2", 0);
    sync(&mut a, &mut b);

    let p = a.project();
    assert_eq!(p, b.project());
    assert_well_formed(&p);
    // Conflicting renames: one wins, the same one everywhere.
    assert!(["Backing (teacher)", "Backing (student)"].contains(&p.tracks["t0"].name.as_str()));
    // Non-conflicting edits all survive.
    assert_eq!(p.tracks["t0"].clips["c0"].start, 960);
    assert_eq!(p.tracks["t1"].gain, 0.5);
    assert!(p.tracks["t1"].clips.contains_key("b-take1"));
    assert_eq!(p.order[0], "t2");
}

fn delete_wins_over_concurrent_edit<D: ProjectDoc>() {
    let (mut a, mut b) = lesson::<D>();
    a.delete_track("t1");
    b.set_gain("t1", 0.25);
    b.add_clip("t1", "b-late", 0, 960);
    sync(&mut a, &mut b);
    let p = a.project();
    assert_eq!(p, b.project());
    assert_well_formed(&p);
    assert!(!p.tracks.contains_key("t1"));
}

fn concurrent_moves_of_one_track<D: ProjectDoc>() {
    let (mut a, mut b) = lesson::<D>();
    a.move_track("t2", 0);
    b.move_track("t2", 1);
    sync(&mut a, &mut b);
    let p = a.project();
    assert_eq!(p, b.project());
    assert_well_formed(&p);
}

fn random_edit<D: ProjectDoc>(d: &mut D, rng: &mut XorShift, peer: u64, n: &mut u32) {
    let p = d.project();
    let tracks: Vec<_> = p.tracks.keys().cloned().collect();
    *n += 1;
    let fresh = format!("p{peer}-{n}");
    if tracks.is_empty() || rng.below(10) == 0 {
        d.add_track(&fresh, "new");
        return;
    }
    let t = &tracks[rng.below(tracks.len())];
    match rng.below(7) {
        0 => d.rename_track(t, &fresh),
        1 => d.set_gain(t, rng.below(100) as f64 / 100.0),
        2 => d.move_track(t, rng.below(tracks.len())),
        3 => d.add_clip(t, &fresh, rng.below(100_000) as i64, 960),
        4 if tracks.len() > 3 => d.delete_track(t),
        _ => {
            let clips: Vec<_> = p.tracks[t].clips.keys().cloned().collect();
            if clips.is_empty() {
                d.add_clip(t, &fresh, 0, 960);
            } else {
                d.move_clip(t, &clips[rng.below(clips.len())], rng.below(100_000) as i64);
            }
        }
    }
}

fn offline_edits_merge_on_reconnect<D: ProjectDoc>() {
    let (mut a, mut b) = lesson::<D>();
    let mut rng = XorShift(42);
    let (mut na, mut nb) = (0, 0);
    // A long offline stretch on both sides.
    for _ in 0..300 {
        random_edit(&mut a, &mut rng, 1, &mut na);
        random_edit(&mut b, &mut rng, 2, &mut nb);
    }
    sync(&mut a, &mut b);
    let p = a.project();
    assert_eq!(p, b.project());
    assert_well_formed(&p);
}

fn three_peers_random_syncs_converge<D: ProjectDoc>() {
    let mut peers: Vec<D> = (1..=3).map(D::new).collect();
    let mut rng = XorShift(7);
    let mut counters = [0u32; 3];
    for _ in 0..600 {
        let i = rng.below(3);
        random_edit(&mut peers[i], &mut rng, i as u64 + 1, &mut counters[i]);
        if rng.below(5) == 0 {
            let j = (i + 1 + rng.below(2)) % 3;
            let (lo, hi) = (i.min(j), i.max(j));
            let (left, right) = peers.split_at_mut(hi);
            sync(&mut left[lo], &mut right[0]);
        }
    }
    // Everyone meets once.
    for _ in 0..2 {
        let (first, rest) = peers.split_at_mut(1);
        for other in rest {
            sync(&mut first[0], other);
        }
    }
    let p = peers[0].project();
    assert_well_formed(&p);
    for other in &peers[1..] {
        assert_eq!(p, other.project());
    }
}

fn undo_only_reverts_own_edits<D: ProjectDoc>() {
    let (mut a, mut b) = lesson::<D>();
    a.set_gain("t0", 0.5);
    b.rename_track("t1", "Bass");
    sync(&mut a, &mut b);
    b.move_clip("t2", "c2", 1920);
    sync(&mut a, &mut b);

    // The teacher undoes their gain change. The student's later edits stay.
    assert!(a.undo());
    sync(&mut a, &mut b);
    let p = b.project();
    assert_eq!(p, a.project());
    assert_eq!(p.tracks["t0"].gain, 1.0);
    assert_eq!(p.tracks["t1"].name, "Bass");
    assert_eq!(p.tracks["t2"].clips["c2"].start, 1920);
}

fn snapshot_restores_the_same_project<D: ProjectDoc>() {
    let (a, _) = lesson::<D>();
    let mut fresh = D::new(9);
    fresh.apply(&a.snapshot());
    assert_eq!(fresh.project(), a.project());
}

macro_rules! for_both {
    ($($scenario:ident),* $(,)?) => {
        mod loro {
            use super::*;
            $( #[test] fn $scenario() { super::$scenario::<LoroProject>() } )*
        }
        mod yrs {
            use super::*;
            $( #[test] fn $scenario() { super::$scenario::<YrsProject>() } )*
        }
    };
}

for_both!(
    concurrent_edits_converge,
    delete_wins_over_concurrent_edit,
    concurrent_moves_of_one_track,
    offline_edits_merge_on_reconnect,
    three_peers_random_syncs_converge,
    undo_only_reverts_own_edits,
    snapshot_restores_the_same_project,
);
