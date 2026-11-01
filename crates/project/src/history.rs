// SPDX-License-Identifier: GPL-3.0-or-later
//! The project's change history, read back (roadmap Phase 4): who
//! changed what and when, and the list of this user's undo steps, so
//! several can be undone at once.
//!
//! Undo itself is Loro's undo manager (see `doc.rs`), which keeps no
//! words for its steps. Each step it pushes is tagged with a new number
//! (its item's value, see [`tag_steps`]), and [`Steps`] keeps that
//! number with the step's label and what it changed, in the same order as
//! the manager's stacks. The tags let a step the labels missed be found
//! and given a plain label, so the list never says the wrong thing.
//!
//! The activity history is worked out from the document itself, so it
//! covers everyone's changes, merged in or made here. Changes are put in
//! causal order and gathered into runs: one person's changes with no
//! more than [`RUN_GAP`] between them. What a run changed is the
//! difference ([`crate::differences`]) between the project before and
//! after it. Runs that change nothing in the song, such as signing in or
//! practice settings, are left out. Changes are timed from when the
//! document started recording time; older ones have none.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use loro::{ID, LoroValue, UndoItemMeta, UndoManager, VersionVector};

use crate::{Difference, Project, ProjectDoc, differences};

/// One person's changes this close together, in seconds, are one run.
pub const RUN_GAP: i64 = 10 * 60;

/// One step on the undo or redo list.
#[derive(Clone, Debug, PartialEq)]
pub struct UndoStep {
    /// In a few words: "Tempo 96 → 120 BPM", "Bass: Volume -6 → -3 dB
    /// and 2 more".
    pub label: String,
    /// Everything it changed.
    pub changes: Vec<Difference>,
    /// When it was made, in milliseconds since 1970.
    pub at: i64,
}

/// A run of one person's changes.
#[derive(Clone, Debug, PartialEq)]
pub struct Activity {
    /// Their name as the project shows it; "Someone" for changes that
    /// say nothing about who made them.
    pub who: String,
    /// Their key, if known.
    pub key: Option<String>,
    /// Made by this user (in this run of the app or another).
    pub mine: bool,
    /// When the run started and ended, in seconds since 1970; 0 if the
    /// changes weren't timed.
    pub from: i64,
    pub to: i64,
    /// What the run changed.
    pub changes: Vec<Difference>,
}

/// The labels of the undo manager's steps, bottom of each stack first.
#[derive(Default)]
pub(crate) struct Steps {
    undo: Vec<(i64, UndoStep)>,
    redo: Vec<(i64, UndoStep)>,
}

/// Has `undo` tag every step it pushes, undo or redo, with a new number.
pub(crate) fn tag_steps(undo: &mut UndoManager) {
    let next = Arc::new(AtomicI64::new(1));
    undo.set_on_push(Some(Box::new(move |_, _, _| {
        meta(next.fetch_add(1, Ordering::Relaxed))
    })));
}

/// A step's label from what it changed.
pub fn label(changes: &[Difference]) -> String {
    let Some(first) = changes.first() else {
        return "An edit".into();
    };
    let one = match first.place.as_str() {
        "Song" | "Tracks" => first.text.clone(),
        place => format!("{place}: {}", first.text),
    };
    match changes.len() {
        1 => one,
        n => format!("{one} and {} more", n - 1),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn tag(value: Option<LoroValue>) -> Option<i64> {
    match value? {
        LoroValue::I64(n) => Some(n),
        _ => None,
    }
}

fn meta(n: i64) -> UndoItemMeta {
    let mut m = UndoItemMeta::new();
    m.set_value(LoroValue::I64(n));
    m
}

impl ProjectDoc {
    /// Writes what differs from `p` as one undoable edit, labelled
    /// `label` on the undo list (for an assistant's request, say), and
    /// kept in the history as its commit message. True if anything
    /// changed.
    pub fn set_labelled(&mut self, p: &Project, label: &str) -> bool {
        let before = self.get();
        let changed = self.write(p);
        self.doc.set_next_commit_message(label);
        self.doc.commit();
        self.note_step(label, differences(&before, &self.get()));
        changed
    }

    /// The steps Undo would take back, the next one first.
    pub fn undo_list(&self) -> Vec<UndoStep> {
        let n = self.undo.undo_count();
        let s = &self.steps.undo;
        let mut v: Vec<UndoStep> = s.iter().rev().map(|(_, st)| st.clone()).collect();
        // Steps from before the labels were kept (none, unless the
        // manager kept more than it said).
        v.resize_with(n.max(v.len()), unnamed);
        v.truncate(n);
        v
    }

    /// The steps Redo would put back, the next one first.
    pub fn redo_list(&self) -> Vec<UndoStep> {
        let n = self.undo.redo_count();
        let mut v: Vec<UndoStep> = (self.steps.redo.iter().rev())
            .map(|(_, st)| st.clone())
            .collect();
        v.resize_with(n.max(v.len()), unnamed);
        v.truncate(n);
        v
    }

    /// Undoes up to `n` steps, each one on its own so they can be redone
    /// one at a time. Returns how many were undone.
    pub fn undo_steps(&mut self, n: usize) -> usize {
        (0..n).take_while(|_| self.undo()).count()
    }

    /// Redoes up to `n` steps. Returns how many were redone.
    pub fn redo_steps(&mut self, n: usize) -> usize {
        (0..n).take_while(|_| self.redo()).count()
    }

    /// Records the label of a step just committed, if the commit made
    /// one, and drops the redo labels the manager dropped with it.
    pub(crate) fn note_step(&mut self, label: &str, changes: Vec<Difference>) {
        if let Some(n) = tag(self.undo.top_undo_value())
            && self.steps.undo.last().map(|s| s.0) != Some(n)
        {
            let step = UndoStep {
                label: label.to_string(),
                changes,
                at: now_ms(),
            };
            self.steps.undo.push((n, step));
        }
        self.trim_steps();
    }

    /// Undo with the labels kept in step: the step undone moves to the
    /// redo list.
    pub(crate) fn undo_labelled(&mut self) -> bool {
        self.note_step("An edit", Vec::new());
        let (before, redo_before) = (self.undo.undo_count(), self.undo.redo_count());
        let done = self.undo.undo().unwrap_or(false);
        if self.undo.undo_count() < before {
            let step = self.steps.undo.pop();
            if self.undo.redo_count() > redo_before
                && let (Some((_, step)), Some(n)) = (step, tag(self.undo.top_redo_value()))
            {
                self.steps.redo.push((n, step));
            }
        }
        self.trim_steps();
        done
    }

    /// Redo, the other way round.
    pub(crate) fn redo_labelled(&mut self) -> bool {
        let (before, undo_before) = (self.undo.redo_count(), self.undo.undo_count());
        let done = self.undo.redo().unwrap_or(false);
        if self.undo.redo_count() < before {
            let step = self.steps.redo.pop();
            if self.undo.undo_count() > undo_before
                && let (Some((_, step)), Some(n)) = (step, tag(self.undo.top_undo_value()))
            {
                self.steps.undo.push((n, step));
            }
        }
        self.trim_steps();
        done
    }

    /// Drops labels for steps the manager no longer has: the oldest,
    /// past its limit, and the redo steps a new edit clears.
    fn trim_steps(&mut self) {
        let keep = |v: &mut Vec<(i64, UndoStep)>, n: usize| {
            let extra = v.len().saturating_sub(n);
            v.drain(..extra);
        };
        keep(&mut self.steps.undo, self.undo.undo_count());
        keep(&mut self.steps.redo, self.undo.redo_count());
        // A top that isn't the label's own means the manager lost track
        // of one of ours: say nothing rather than the wrong thing.
        let tops = [
            (tag(self.undo.top_undo_value()), &mut self.steps.undo),
            (tag(self.undo.top_redo_value()), &mut self.steps.redo),
        ];
        for (top, v) in tops {
            if let (Some(t), Some((n, st))) = (top, v.last_mut())
                && *n != t
            {
                *st = unnamed();
            }
        }
    }

    /// Who changed what and when, newest first, for at most the last
    /// `limit` runs.
    pub fn activity(&self, limit: usize) -> Vec<Activity> {
        let (peers, names) = self.peer_keys();
        let my_key = self.my_key().map(str::to_string);
        let me = self.doc.peer_id();
        let person = |peer: u64| -> (String, Option<String>, bool) {
            match peers.get(&peer) {
                Some(k) => (
                    names.get(k).cloned().unwrap_or_else(|| "Someone".into()),
                    Some(k.clone()),
                    my_key.as_deref() == Some(k.as_str()),
                ),
                None if peer == me => ("You".into(), None, true),
                None => ("Someone".into(), None, false),
            }
        };

        // Every change, in an order that keeps each after what it built on.
        let mut changes = Vec::new();
        for (&peer, &end) in self.doc.oplog_vv().iter() {
            let mut counter = 0;
            while counter < end {
                let Some(c) = self.doc.get_change(ID::new(peer, counter)) else {
                    break;
                };
                let len = c.len.max(1) as i32;
                changes.push((
                    c.lamport,
                    peer,
                    c.id.counter,
                    c.id.counter + len,
                    c.timestamp,
                ));
                counter = c.id.counter + len;
            }
        }
        changes.sort_by_key(|c| (c.0, c.1, c.2));

        // Runs of one person's changes, with the version after each.
        struct Run {
            peer_who: (String, Option<String>, bool),
            from: i64,
            to: i64,
            end: VersionVector,
        }
        let mut runs: Vec<Run> = Vec::new();
        let mut vv = VersionVector::new();
        for &(_, peer, _, end, at) in &changes {
            vv.extend_to_include_last_id(ID::new(peer, end - 1));
            let who = person(peer);
            match runs.last_mut() {
                Some(r) if r.peer_who == who && (at - r.to).abs() <= RUN_GAP => {
                    r.to = r.to.max(at);
                    r.end = vv.clone();
                }
                _ => runs.push(Run {
                    peer_who: who,
                    from: at,
                    to: at,
                    end: vv.clone(),
                }),
            }
        }

        // What each of the last runs changed, walking one copy forward.
        // Runs that change nothing in the song are dropped, so look
        // further back than `limit`.
        let first = runs.len().saturating_sub(limit.max(1) * 4);
        let walk = ProjectDoc::wrap(self.doc.fork());
        let read = |v: &VersionVector| -> Project {
            match walk.doc.checkout(&self.doc.vv_to_frontiers(v)) {
                Ok(()) => walk.get(),
                Err(_) => Project::default(),
            }
        };
        let mut before = match first {
            0 => Project::default(),
            i => read(&runs[i - 1].end),
        };
        let mut out = Vec::new();
        for r in &runs[first..] {
            let after = read(&r.end);
            let changes = differences(&before, &after);
            if !changes.is_empty() {
                out.push(Activity {
                    who: r.peer_who.0.clone(),
                    key: r.peer_who.1.clone(),
                    mine: r.peer_who.2,
                    from: r.from,
                    to: r.to,
                    changes,
                });
            }
            before = after;
        }
        out.reverse();
        out.truncate(limit);
        out
    }
}

fn unnamed() -> UndoStep {
    UndoStep {
        label: "An edit".into(),
        changes: Vec::new(),
        at: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Identity, Marker, Role};

    fn edit(doc: &mut ProjectDoc, f: impl FnOnce(&mut Project)) {
        let mut p = doc.get();
        f(&mut p);
        doc.set(&p);
    }

    fn marker(id: &str, bar: i64) -> Marker {
        Marker {
            id: id.into(),
            bar,
            name: id.into(),
        }
    }

    fn labels(v: &[UndoStep]) -> Vec<&str> {
        v.iter().map(|s| s.label.as_str()).collect()
    }

    #[test]
    fn each_edit_is_a_labelled_step_newest_first() {
        let mut doc = ProjectDoc::new(1);
        edit(&mut doc, |p| p.tempo_bpm = 96.0);
        edit(&mut doc, |p| p.markers.push(marker("Intro", 1)));
        edit(&mut doc, |p| {
            p.markers[0].bar = 3;
            p.tempo_bpm = 100.0;
        });
        assert_eq!(
            labels(&doc.undo_list()),
            [
                "Tempo 96 → 100 BPM and 1 more",
                "Markers: Added “Intro” at bar 1",
                "Tempo 120 → 96 BPM",
            ]
        );
        assert_eq!(doc.undo_list()[0].changes.len(), 2);
        assert!(doc.redo_list().is_empty());
    }

    #[test]
    fn undoing_several_moves_them_to_redo_and_back() {
        let mut doc = ProjectDoc::new(1);
        for bpm in [90.0, 100.0, 110.0, 130.0] {
            edit(&mut doc, |p| p.tempo_bpm = bpm);
        }
        assert_eq!(doc.undo_steps(3), 3);
        assert_eq!(doc.get().tempo_bpm, 90.0);
        assert_eq!(labels(&doc.undo_list()), ["Tempo 120 → 90 BPM"]);
        assert_eq!(
            labels(&doc.redo_list()),
            [
                "Tempo 90 → 100 BPM",
                "Tempo 100 → 110 BPM",
                "Tempo 110 → 130 BPM",
            ]
        );
        assert_eq!(doc.redo_steps(2), 2);
        assert_eq!(doc.get().tempo_bpm, 110.0);
        assert_eq!(labels(&doc.redo_list()), ["Tempo 110 → 130 BPM"]);
        assert_eq!(doc.undo_list().len(), 3);
        // Asking for more than there are takes what there is.
        assert_eq!(doc.undo_steps(10), 3);
        assert_eq!(doc.get().tempo_bpm, 120.0);
        assert!(!doc.can_undo());
    }

    #[test]
    fn a_new_edit_clears_the_redo_list() {
        let mut doc = ProjectDoc::new(1);
        edit(&mut doc, |p| p.tempo_bpm = 90.0);
        edit(&mut doc, |p| p.tempo_bpm = 100.0);
        doc.undo();
        assert_eq!(doc.redo_list().len(), 1);
        edit(&mut doc, |p| p.markers.push(marker("Solo", 9)));
        assert!(doc.redo_list().is_empty());
        assert_eq!(
            labels(&doc.undo_list()),
            ["Markers: Added “Solo” at bar 9", "Tempo 120 → 90 BPM"]
        );
    }

    #[test]
    fn an_edit_can_bring_its_own_label() {
        let mut doc = ProjectDoc::new(1);
        let mut p = doc.get();
        p.tempo_bpm = 80.0;
        doc.set_labelled(&p, "Assistant: slow it down");
        assert_eq!(labels(&doc.undo_list()), ["Assistant: slow it down"]);
        assert_eq!(doc.undo_list()[0].changes[0].text, "Tempo 120 → 80 BPM");
    }

    #[test]
    fn steps_merged_in_from_someone_else_are_not_on_my_list() {
        let mut mine = ProjectDoc::new(1);
        edit(&mut mine, |p| p.tempo_bpm = 90.0);
        let mut theirs = ProjectDoc::from_bytes(&mine.to_bytes(), 2).unwrap();
        edit(&mut theirs, |p| p.markers.push(marker("Bridge", 5)));
        mine.merge(&theirs.to_bytes()).unwrap();
        assert_eq!(labels(&mine.undo_list()), ["Tempo 120 → 90 BPM"]);
        assert!(mine.undo());
        assert_eq!(mine.get().markers.len(), 1, "their marker stays");
    }

    #[test]
    fn activity_says_who_changed_what() {
        let andy = Identity::generate("Andy").unwrap();
        let mut mine = ProjectDoc::new(1);
        mine.sign_in(&andy);
        edit(&mut mine, |p| p.tempo_bpm = 96.0);
        edit(&mut mine, |p| p.markers.push(marker("Verse", 1)));

        let pat = Identity::generate("Pat").unwrap();
        let mut theirs = ProjectDoc::from_bytes(&mine.to_bytes(), 2).unwrap();
        theirs.sign_in(&pat);
        let key = theirs.my_key().unwrap().to_string();
        mine.merge(&theirs.to_bytes()).unwrap();
        mine.set_role(&key, Some(Role::Editor)).unwrap();
        theirs.merge(&mine.to_bytes()).unwrap();
        edit(&mut theirs, |p| p.markers[0].bar = 5);
        mine.merge(&theirs.to_bytes()).unwrap();

        let a = mine.activity(10);
        assert_eq!(a.len(), 2, "{a:?}");
        assert_eq!(a[0].who, "Pat");
        assert!(!a[0].mine);
        assert_eq!(a[0].changes[0].text, "Moved “Verse” from bar 1 to bar 5");
        assert_eq!(a[1].who, "Andy");
        assert!(a[1].mine);
        let texts: Vec<&str> = a[1].changes.iter().map(|d| d.text.as_str()).collect();
        assert_eq!(texts, ["Tempo 120 → 96 BPM", "Added “Verse” at bar 1"]);
        assert!(a[1].from > 0, "changes are timed");
        // The newest run alone.
        assert_eq!(mine.activity(1).len(), 1);
        assert_eq!(mine.activity(1)[0].who, "Pat");
    }

    #[test]
    fn activity_before_signing_in_is_yours() {
        let mut doc = ProjectDoc::new(7);
        edit(&mut doc, |p| p.tempo_bpm = 140.0);
        let a = doc.activity(5);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].who, "You");
        assert!(a[0].mine);
    }

    #[test]
    fn activity_survives_saving_and_opening() {
        let mut doc = ProjectDoc::new(1);
        edit(&mut doc, |p| p.tempo_bpm = 100.0);
        let opened = ProjectDoc::from_bytes(&doc.to_bytes(), 2).unwrap();
        let a = opened.activity(5);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].who, "Someone");
        assert_eq!(a[0].changes[0].text, "Tempo 120 → 100 BPM");
    }
}
