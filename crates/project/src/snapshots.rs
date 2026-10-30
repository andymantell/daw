// SPDX-License-Identifier: GPL-3.0-or-later
//! Snapshots (roadmap 1.8): a name for the project as it was at a
//! moment, such as "Lesson 3 end", and restoring it later.
//!
//! The document keeps its whole history, so a snapshot is only a name
//! for one of its versions: the `snapshots` map holds, by id, a map of
//! `name`, `author`, `created` and `version` (the document's frontiers
//! then, encoded). They travel with the document, to bundles and shared
//! folders, like everything else in it.
//!
//! Restoring reads the project at that version and writes what differs
//! as one ordinary, undoable edit. It doesn't wind the history back, so
//! it merges with other people's changes like any edit, and they can
//! still see and restore what came after.
//!
//! Some things are left as they are now: comments, so restoring never
//! brings back the words of one deleted since (saved files keep only
//! blanks for those, see `doc.rs`) nor drops ones written after;
//! assignments, so going back to how the song was at a lesson doesn't
//! drop what was set since; and each person's practice settings and
//! practice log, which aren't the song's.

use loro::{Frontiers, LoroMap, LoroValue};

use crate::doc::QUIET;
use crate::{Difference, Project, ProjectDoc, differences};

/// Where snapshots live in the document.
const SNAPSHOTS: &str = "snapshots";

/// A snapshot's name is cut to this many characters.
pub const MAX_SNAPSHOT_NAME: usize = 80;

/// A named version of the project.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub id: String,
    pub name: String,
    /// Who took it: the member's public key, as hex, or empty.
    pub author: String,
    /// When, in milliseconds since 1970.
    pub created: i64,
    /// Whether this copy has the version it names (a damaged or
    /// tampered copy might not), so it can be restored.
    pub restorable: bool,
}

impl ProjectDoc {
    /// Names the project as it is now. Not undoable: undo is for edits
    /// to the song. Returns the new snapshot's id.
    pub fn take_snapshot(&mut self, name: &str, author: &str, created: i64) -> String {
        // Anything written but not yet committed belongs in it.
        self.doc.commit();
        let version = self.doc.oplog_frontiers().encode();
        let id = crate::new_id();
        let all = self.doc.get_map(SNAPSHOTS);
        if let Ok(m) = all.insert_container(&id, LoroMap::new()) {
            let _ = m.insert("name", clean(name).as_str());
            let _ = m.insert("author", author);
            let _ = m.insert("created", created);
            let _ = m.insert("version", version);
        }
        self.commit_snapshots();
        id
    }

    /// Every snapshot, newest first.
    pub fn snapshots(&self) -> Vec<Snapshot> {
        let LoroValue::Map(all) = self.doc.get_map(SNAPSHOTS).get_deep_value() else {
            return Vec::new();
        };
        let mut v: Vec<Snapshot> = (all.iter())
            .filter_map(|(id, m)| {
                let LoroValue::Map(m) = m else { return None };
                let text = |k: &str| match m.get(k) {
                    Some(LoroValue::String(s)) => s.to_string(),
                    _ => String::new(),
                };
                Some(Snapshot {
                    id: id.to_string(),
                    name: text("name"),
                    author: text("author"),
                    created: match m.get("created") {
                        Some(LoroValue::I64(t)) => *t,
                        _ => 0,
                    },
                    restorable: self.version_of(id).is_some(),
                })
            })
            .collect();
        v.sort_by(|a, b| b.created.cmp(&a.created).then(b.id.cmp(&a.id)));
        v
    }

    /// Gives a snapshot a new name. False if there is no such snapshot.
    pub fn rename_snapshot(&mut self, id: &str, name: &str) -> bool {
        let Some(m) = self.snapshot_map(id) else {
            return false;
        };
        let _ = m.insert("name", clean(name).as_str());
        self.commit_snapshots();
        true
    }

    /// Forgets a snapshot. The project's history is untouched: only the
    /// name goes. False if there was no such snapshot.
    pub fn delete_snapshot(&mut self, id: &str) -> bool {
        let all = self.doc.get_map(SNAPSHOTS);
        if all.get(id).is_none() {
            return false;
        }
        let _ = all.delete(id);
        self.commit_snapshots();
        true
    }

    /// The project as it was when the snapshot was taken, comments and
    /// assignments aside: those are the ones there are now.
    pub fn snapshot_project(&self, id: &str) -> Option<Project> {
        let version = self.version_of(id)?;
        let then = self.doc.fork_at(&version).ok()?;
        let mut p = ProjectDoc::wrap(then).get();
        let now = self.get();
        p.comments = now.comments;
        p.assignments = now.assignments;
        Some(p)
    }

    /// What changed from snapshot `from` to snapshot `to`, or to the
    /// project as it is now when `to` is `None`. `None` if either can't
    /// be read from this copy.
    pub fn snapshot_differences(&self, from: &str, to: Option<&str>) -> Option<Vec<Difference>> {
        let before = self.snapshot_project(from)?;
        let after = match to {
            Some(id) => self.snapshot_project(id)?,
            None => self.get(),
        };
        Some(differences(&before, &after))
    }

    /// Puts the project back as it was at the snapshot, as one undoable
    /// edit, and returns it. `None` if the snapshot can't be restored.
    pub fn restore_snapshot(&mut self, id: &str) -> Option<Project> {
        let p = self.snapshot_project(id)?;
        self.set(&p);
        Some(p)
    }

    /// The assets (clips' audio and frozen audio) that the project uses
    /// now or that any snapshot would bring back, so a bundle can carry
    /// all of them. Sorted, once each.
    pub fn assets_in_use(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        let mut add = |p: &Project| {
            v.extend(p.tracks.iter().flat_map(|t| {
                (t.clips.iter().map(|c| c.asset.clone()))
                    .chain(t.frozen.iter().map(|f| f.asset.clone()))
                    .chain(t.effects.iter().flat_map(|e| e.assets()).map(String::from))
                    .chain(
                        t.instrument
                            .iter()
                            .flat_map(|e| e.assets())
                            .map(String::from),
                    )
            }))
        };
        add(&self.get());
        for s in self.snapshots() {
            if let Some(p) = self.snapshot_project(&s.id) {
                add(&p);
            }
        }
        v.sort();
        v.dedup();
        v
    }

    fn snapshot_map(&self, id: &str) -> Option<LoroMap> {
        self.doc
            .get_map(SNAPSHOTS)
            .get(id)?
            .into_container()
            .ok()?
            .into_map()
            .ok()
    }

    /// The version a snapshot names, if this copy has it.
    fn version_of(&self, id: &str) -> Option<Frontiers> {
        let m = self.snapshot_map(id)?;
        let LoroValue::Binary(b) = m.get("version")?.into_value().ok()? else {
            return None;
        };
        let f = Frontiers::decode(&b).ok()?;
        // Every change it names must be here.
        let vv = self.doc.oplog_vv();
        let all_here = f.iter().all(|id| vv.includes_id(id));
        all_here.then_some(f)
    }

    fn commit_snapshots(&mut self) {
        self.doc.set_next_commit_origin(QUIET);
        self.doc.commit();
    }
}

/// A name with no line breaks or surrounding space, not too long.
fn clean(name: &str) -> String {
    let one_line: String = name
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    one_line.trim().chars().take(MAX_SNAPSHOT_NAME).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Clip, Comment, Effect, EffectState, Marker, Track};

    fn clip(id: &str, asset: &str) -> Clip {
        Clip {
            id: id.into(),
            asset: asset.into(),
            name: "take".into(),
            start: 0,
            offset: 0,
            len: 100,
            take: None,
            muted: false,
            seq: 0,
            ..Default::default()
        }
    }

    fn comment(id: &str, text: &str) -> Comment {
        Comment {
            id: id.into(),
            author: String::new(),
            parent: None,
            track: None,
            start: 0,
            end: None,
            text: text.into(),
            resolved: false,
            created: 1,
        }
    }

    /// A project with one track, one clip and a marker.
    fn lesson() -> (ProjectDoc, Project) {
        let mut doc = ProjectDoc::new(1);
        let mut p = Project::default();
        let mut t = Track::new("t1".into(), "Guitar".into());
        t.clips.push(clip("c1", "aaaa.wav"));
        p.tracks.push(t);
        p.markers.push(Marker {
            id: "m1".into(),
            bar: 1,
            name: "Intro".into(),
        });
        p.notes = "Practise the G chord".into();
        doc.set(&p);
        (doc, p)
    }

    #[test]
    fn restoring_puts_the_song_back_as_one_undoable_edit() {
        let (mut doc, p) = lesson();
        let id = doc.take_snapshot("Lesson 3 end", "", 10);
        let mut q = p.clone();
        q.tempo_bpm = 90.0;
        q.tracks[0].name = "Lead".into();
        q.tracks[0].clips.push(clip("c2", "bbbb.wav"));
        q.tracks.push(Track::new("t2".into(), "Bass".into()));
        q.markers.clear();
        q.notes = "Now the C chord".into();
        doc.set(&q);

        assert_eq!(doc.restore_snapshot(&id), Some(p.clone()));
        assert_eq!(doc.get(), p);
        // Undo takes the restore back, in one step.
        assert!(doc.undo());
        assert_eq!(doc.get(), q);
        assert!(doc.redo());
        assert_eq!(doc.get(), p);
        // Taking a snapshot is not itself an undo step.
        assert!(doc.undo());
        assert!(doc.undo());
        assert!(doc.undo());
        assert_eq!(doc.get(), Project::default());
        assert_eq!(doc.snapshots().len(), 1);
    }

    #[test]
    fn snapshots_are_listed_newest_first_and_can_be_renamed_and_forgotten() {
        let (mut doc, _) = lesson();
        let a = doc.take_snapshot("Lesson 1", "k1", 10);
        let b = doc.take_snapshot("  Lesson\n2  ", "k2", 20);
        let all = doc.snapshots();
        assert_eq!(
            all.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["Lesson 2", "Lesson 1"]
        );
        assert_eq!(all[0].author, "k2");
        assert!(all.iter().all(|s| s.restorable));
        assert!(doc.rename_snapshot(&a, "First lesson"));
        assert!(doc.delete_snapshot(&b));
        assert!(!doc.delete_snapshot(&b));
        assert!(!doc.rename_snapshot("nope", "x"));
        let all = doc.snapshots();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "First lesson");
        let long = "x".repeat(200);
        doc.rename_snapshot(&a, &long);
        assert_eq!(doc.snapshots()[0].name.chars().count(), MAX_SNAPSHOT_NAME);
    }

    #[test]
    fn restoring_keeps_comments_as_they_are() {
        let (mut doc, mut p) = lesson();
        p.comments.push(comment("x1", "call me on 07700 900123"));
        doc.set(&p);
        let id = doc.take_snapshot("With a comment", "", 10);
        p.comments.clear();
        p.comments.push(comment("x2", "nice bend"));
        p.tempo_bpm = 100.0;
        doc.set(&p);
        // Saved and opened again, so the deleted words are blanked out of
        // the history: the snapshot still restores.
        let bytes = doc.to_bytes();
        assert!(!bytes.windows(5).any(|w| w == b"07700"));
        let mut doc = ProjectDoc::from_bytes(&bytes, 2).unwrap();
        let back = doc.restore_snapshot(&id).unwrap();
        assert_eq!(back.tempo_bpm, 120.0);
        assert_eq!(back.comments, vec![comment("x2", "nice bend")]);
        assert_eq!(doc.get().comments, back.comments);
    }

    #[test]
    fn a_restore_merges_with_someone_elses_edit() {
        let (mut mine, p) = lesson();
        let id = mine.take_snapshot("Before", "", 10);
        let mut q = p.clone();
        q.tempo_bpm = 80.0;
        mine.set(&q);
        let mut theirs = ProjectDoc::from_bytes(&mine.to_bytes(), 2).unwrap();
        // They add a marker while I restore.
        let mut r = theirs.get();
        r.markers.push(Marker {
            id: "m2".into(),
            bar: 5,
            name: "Solo".into(),
        });
        theirs.set(&r);
        mine.restore_snapshot(&id).unwrap();
        mine.merge(&theirs.to_bytes()).unwrap();
        theirs.merge(&mine.to_bytes()).unwrap();
        let both = mine.get();
        assert_eq!(both, theirs.get());
        assert_eq!(both.tempo_bpm, 120.0, "the restore");
        assert_eq!(both.markers.len(), 2, "their marker");
        // They see my snapshot too, and can restore it themselves.
        assert_eq!(theirs.snapshots().len(), 1);
        assert!(theirs.snapshot_project(&id).is_some());
    }

    #[test]
    fn plugin_settings_come_back_with_the_snapshot() {
        let (mut doc, mut p) = lesson();
        let state = |b: u8| {
            Some(EffectState {
                component: vec![b; 4],
                controller: None,
            })
        };
        p.tracks[0].effects.push(Effect {
            id: "e1".into(),
            name: "Drive".into(),
            class: vec![7; 16],
            bundle: String::new(),
            bypass: false,
            state: state(1),
            ..Default::default()
        });
        doc.set_quietly(&p);
        let id = doc.take_snapshot("Clean", "", 10);
        p.tracks[0].effects[0].state = state(2);
        doc.set_quietly(&p);
        let back = doc.restore_snapshot(&id).unwrap();
        assert_eq!(back.tracks[0].effects[0].state, state(1));
        assert_eq!(doc.get().tracks[0].effects[0].state, state(1));
    }

    #[test]
    fn snapshots_can_be_compared_with_each_other_and_with_now() {
        let (mut doc, mut p) = lesson();
        let a = doc.take_snapshot("Lesson 1", "", 10);
        p.tempo_bpm = 96.0;
        doc.set(&p);
        let b = doc.take_snapshot("Lesson 2", "", 20);
        p.markers.clear();
        doc.set(&p);
        let text = |d: Vec<Difference>| d.into_iter().map(|d| d.text).collect::<Vec<_>>();
        assert_eq!(
            text(doc.snapshot_differences(&a, Some(&b)).unwrap()),
            ["Tempo 120 → 96 BPM"]
        );
        assert_eq!(
            text(doc.snapshot_differences(&b, None).unwrap()),
            ["Removed “Intro” (bar 1)"]
        );
        assert_eq!(doc.snapshot_differences(&a, None).unwrap().len(), 2);
        assert!(doc.snapshot_differences("nope", None).is_none());
    }

    #[test]
    fn bundles_carry_audio_that_snapshots_would_bring_back() {
        let (mut doc, mut p) = lesson();
        doc.take_snapshot("Old take", "", 10);
        p.tracks[0].clips = vec![clip("c9", "cccc.wav")];
        doc.set(&p);
        assert_eq!(doc.assets_in_use(), ["aaaa.wav", "cccc.wav"]);
    }

    #[test]
    fn a_snapshot_naming_a_version_this_copy_lacks_cant_be_restored() {
        let (mut doc, _) = lesson();
        let id = doc.take_snapshot("Real", "", 10);
        let m = doc.snapshot_map(&id).unwrap();
        let bogus = Frontiers::from_id(loro::ID::new(999, 5));
        let _ = m.insert("version", bogus.encode());
        doc.doc.commit();
        assert!(!doc.snapshots()[0].restorable);
        assert!(doc.restore_snapshot(&id).is_none());
    }
}
