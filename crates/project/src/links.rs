// SPDX-License-Identifier: GPL-3.0-or-later
//! Linked copies: instances of one clip.
//!
//! Copying a clip makes a linked copy by default: the copy carries the
//! same `link` as the clip it came from, and what is inside the clip is
//! shared by every clip with that link. A note clip's instances share
//! their notes, controller lanes, loop, key and name; an audio clip's
//! share their audio, name, gain, volume line, transpose and pitch
//! edits. Where each instance sits, how long it is (its trim), its fades
//! and its mute stay its own, so a copy can be split, trimmed or moved
//! without touching the others.
//!
//! The content is kept in every instance, so the document needs no new
//! layout and older projects open as they were (no clip has a link).
//! The app hands each edit to [`sync_links`] before writing it: an
//! instance whose shared content changed is copied to the rest of its
//! group, so the whole change is one undo step. Pattern clips already
//! link through their pattern and are left out.

use std::collections::{HashMap, HashSet};

use crate::{Clip, NoteClip, Project, new_id};

/// Whether two note clips hold the same shared content.
fn same_notes(a: &NoteClip, b: &NoteClip) -> bool {
    a.notes == b.notes
        && a.controllers == b.controllers
        && a.loop_len == b.loop_len
        && a.scale == b.scale
        && a.name == b.name
}

fn copy_notes(from: &NoteClip, to: &mut NoteClip) {
    to.notes = from.notes.clone();
    to.controllers = from.controllers.clone();
    to.loop_len = from.loop_len;
    to.scale = from.scale;
    to.name = from.name.clone();
}

/// Whether two audio clips hold the same shared content.
fn same_audio(a: &Clip, b: &Clip) -> bool {
    a.asset == b.asset
        && a.name == b.name
        && a.gain_db == b.gain_db
        && a.envelope == b.envelope
        && a.cents == b.cents
        && a.move_formants == b.move_formants
        && a.pitch_notes == b.pitch_notes
}

fn copy_audio(from: &Clip, to: &mut Clip) {
    to.asset = from.asset.clone();
    to.name = from.name.clone();
    to.gain_db = from.gain_db;
    to.envelope = from.envelope.clone();
    to.cents = from.cents;
    to.move_formants = from.move_formants;
    to.pitch_notes = from.pitch_notes.clone();
}

/// Brings every linked group in `after` into step with the instance
/// that was edited since `before`: its shared content goes to the rest
/// of its group. A clip only counts as edited if it was there before,
/// on a track that didn't switch version (switching shows another
/// version's clips; it doesn't edit them). If several instances changed
/// differently, the first (by track, then clip) wins. True if anything
/// was copied.
pub fn sync_links(before: &Project, after: &mut Project) -> bool {
    let switched: HashSet<String> = (after.tracks.iter())
        .filter(|t| {
            (before.tracks.iter())
                .find(|b| b.id == t.id)
                .is_some_and(|b| b.version != t.version)
        })
        .map(|t| t.id.clone())
        .collect();
    let steady = || (before.tracks.iter()).filter(|t| !switched.contains(t.id.as_str()));
    let old_audio: HashMap<&str, &Clip> = steady()
        .flat_map(|t| &t.clips)
        .filter(|c| c.link.is_some())
        .map(|c| (c.id.as_str(), c))
        .collect();
    let old_notes: HashMap<&str, &NoteClip> = steady()
        .flat_map(|t| &t.note_clips)
        .filter(|c| c.link.is_some() && c.pattern.is_none())
        .map(|c| (c.id.as_str(), c))
        .collect();
    let mut changed = false;

    // Audio clips.
    let mut groups: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
    for (ti, t) in after.tracks.iter().enumerate() {
        for (ci, c) in t.clips.iter().enumerate() {
            if let Some(l) = &c.link {
                groups.entry(l.clone()).or_default().push((ti, ci));
            }
        }
    }
    for members in groups.values().filter(|m| m.len() > 1) {
        let source = members.iter().copied().find(|&(ti, ci)| {
            let c = &after.tracks[ti].clips[ci];
            !switched.contains(after.tracks[ti].id.as_str())
                && old_audio
                    .get(c.id.as_str())
                    .is_some_and(|o| !same_audio(o, c))
        });
        let Some((sti, sci)) = source else { continue };
        let src = after.tracks[sti].clips[sci].clone();
        for &(ti, ci) in members {
            let c = &mut after.tracks[ti].clips[ci];
            if !same_audio(&src, c) {
                copy_audio(&src, c);
                changed = true;
            }
        }
    }

    // Note clips.
    let mut groups: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
    for (ti, t) in after.tracks.iter().enumerate() {
        for (ci, c) in t.note_clips.iter().enumerate() {
            if let (Some(l), None) = (&c.link, &c.pattern) {
                groups.entry(l.clone()).or_default().push((ti, ci));
            }
        }
    }
    for members in groups.values().filter(|m| m.len() > 1) {
        let source = members.iter().copied().find(|&(ti, ci)| {
            let c = &after.tracks[ti].note_clips[ci];
            !switched.contains(after.tracks[ti].id.as_str())
                && old_notes
                    .get(c.id.as_str())
                    .is_some_and(|o| !same_notes(o, c))
        });
        let Some((sti, sci)) = source else { continue };
        let src = after.tracks[sti].note_clips[sci].clone();
        for &(ti, ci) in members {
            let c = &mut after.tracks[ti].note_clips[ci];
            if !same_notes(&src, c) {
                copy_notes(&src, c);
                changed = true;
            }
        }
    }
    changed
}

/// A group of linked clips, as found in a project.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkGroup {
    /// The link the clips share; for pattern clips, `pattern:` and the
    /// pattern's id.
    pub link: String,
    /// The clips' ids, in time order (audio by frame, notes by tick).
    pub clips: Vec<String>,
    /// Whether they are note clips (else audio).
    pub notes: bool,
}

/// The groups of two or more clips that are instances of one clip:
/// linked copies, and note clips playing the same pattern. In the order
/// their first clip is found.
pub fn link_groups(p: &Project) -> Vec<LinkGroup> {
    let mut out: Vec<LinkGroup> = Vec::new();
    let mut at: HashMap<(bool, String), usize> = HashMap::new();
    let mut add = |out: &mut Vec<LinkGroup>, notes: bool, key: String, id: &str, pos: i64| {
        let i = *at.entry((notes, key.clone())).or_insert_with(|| {
            out.push(LinkGroup {
                link: key,
                clips: Vec::new(),
                notes,
            });
            out.len() - 1
        });
        out[i].clips.push(format!("{pos:020}\u{0}{id}"));
    };
    for t in &p.tracks {
        for c in &t.clips {
            if let Some(l) = &c.link {
                add(&mut out, false, l.clone(), &c.id, c.start);
            }
        }
        for c in &t.note_clips {
            let key = match (&c.pattern, &c.link) {
                (Some(pat), _) => format!("pattern:{pat}"),
                (None, Some(l)) => l.clone(),
                (None, None) => continue,
            };
            add(&mut out, true, key, &c.id, c.start);
        }
    }
    out.retain(|g| g.clips.len() > 1);
    for g in &mut out {
        g.clips.sort();
        for c in &mut g.clips {
            *c = c.split('\u{0}').nth(1).unwrap_or_default().to_string();
        }
    }
    out
}

/// Gives clip `id` a link, making one if it has none, and returns it:
/// for a copy about to be made of it.
pub fn link_of(p: &mut Project, id: &str) -> Option<String> {
    for t in &mut p.tracks {
        if let Some(c) = t.clips.iter_mut().find(|c| c.id == id) {
            return Some(c.link.get_or_insert_with(new_id).clone());
        }
        if let Some(c) = t.note_clips.iter_mut().find(|c| c.id == id) {
            if c.pattern.is_some() {
                return None;
            }
            return Some(c.link.get_or_insert_with(new_id).clone());
        }
    }
    None
}

/// Breaks clip `id` off from its linked copies, so it can be edited on
/// its own; a copy left alone in its group stops being linked too. True
/// if it was linked.
pub fn unlink(p: &mut Project, id: &str) -> bool {
    let mut link = None;
    for t in &mut p.tracks {
        if let Some(c) = t.clips.iter_mut().find(|c| c.id == id) {
            link = c.link.take();
        } else if let Some(c) = t.note_clips.iter_mut().find(|c| c.id == id) {
            link = c.link.take();
        }
        if link.is_some() {
            break;
        }
    }
    let Some(link) = link else { return false };
    let mut left: Vec<&mut Option<String>> = Vec::new();
    for t in &mut p.tracks {
        left.extend(
            (t.clips.iter_mut())
                .map(|c| &mut c.link)
                .filter(|l| l.as_deref() == Some(link.as_str())),
        );
        left.extend(
            (t.note_clips.iter_mut())
                .map(|c| &mut c.link)
                .filter(|l| l.as_deref() == Some(link.as_str())),
        );
    }
    if let [only] = left.as_mut_slice() {
        **only = None;
    }
    true
}

/// How many clips share clip `id`'s link (its pattern, for a pattern
/// clip), itself included: 1 for a clip of its own.
pub fn instances(p: &Project, id: &str) -> usize {
    link_groups(p)
        .iter()
        .find(|g| g.clips.iter().any(|c| c == id))
        .map_or(1, |g| g.clips.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Note, Track, TrackKind};

    fn note(id: &str, start: i64, pitch: u8) -> Note {
        Note {
            id: id.into(),
            ..Note::new(start, 240, pitch, 100)
        }
    }

    fn song() -> Project {
        let mut p = Project::default();
        let mut t = Track::new("t".into(), "Keys".into());
        t.kind = TrackKind::Instrument;
        let mut a = NoteClip::new("a".into(), "Riff".into(), 0, 3840);
        a.notes = vec![note("n1", 0, 60)];
        a.link = Some("L".into());
        let mut b = a.clone();
        b.id = "b".into();
        b.start = 3840;
        let mut c = a.clone();
        c.id = "c".into();
        c.start = 7680;
        c.link = None;
        t.note_clips = vec![a, b, c];
        let mut audio = Track::new("v".into(), "Vocal".into());
        let x = Clip {
            id: "x".into(),
            asset: "voice.wav".into(),
            name: "Voice".into(),
            len: 48_000,
            link: Some("A".into()),
            ..Default::default()
        };
        let y = Clip {
            id: "y".into(),
            start: 96_000,
            ..x.clone()
        };
        audio.clips = vec![x, y];
        p.tracks = vec![t, audio];
        p
    }

    #[test]
    fn an_edit_to_one_note_clip_goes_to_its_linked_copies_only() {
        let before = song();
        let mut after = before.clone();
        after.tracks[0].note_clips[1]
            .notes
            .push(note("n2", 480, 64));
        after.tracks[0].note_clips[1].loop_len = Some(960);
        assert!(sync_links(&before, &mut after));
        let clips = &after.tracks[0].note_clips;
        assert_eq!(clips[0].notes.len(), 2);
        assert_eq!(clips[0].loop_len, Some(960));
        // Where each sits is its own; the unlinked one is untouched.
        assert_eq!(clips[0].start, 0);
        assert_eq!(clips[2].notes.len(), 1);
        // Nothing more to do.
        let again = after.clone();
        assert!(!sync_links(&again, &mut after));
    }

    #[test]
    fn moving_or_trimming_or_muting_an_instance_stays_its_own() {
        let before = song();
        let mut after = before.clone();
        let b = &mut after.tracks[0].note_clips[1];
        b.start = 9600;
        b.len = 1920;
        b.offset = 480;
        b.muted = true;
        after.tracks[1].clips[1].fade_in.len = 400;
        after.tracks[1].clips[1].len = 20_000;
        assert!(!sync_links(&before, &mut after));
        assert_eq!(after.tracks[0].note_clips[0].len, 3840);
        assert!(!after.tracks[0].note_clips[0].muted);
        assert_eq!(after.tracks[1].clips[0].fade_in.len, 0);
    }

    #[test]
    fn audio_copies_share_gain_and_transpose() {
        let before = song();
        let mut after = before.clone();
        after.tracks[1].clips[0].gain_db = -3.0;
        after.tracks[1].clips[0].cents = -600;
        assert!(sync_links(&before, &mut after));
        assert_eq!(after.tracks[1].clips[1].gain_db, -3.0);
        assert_eq!(after.tracks[1].clips[1].cents, -600);
    }

    #[test]
    fn a_new_copy_never_overwrites_its_group() {
        let before = song();
        let mut after = before.clone();
        let mut d = after.tracks[0].note_clips[0].clone();
        d.id = "d".into();
        d.notes.clear();
        after.tracks[0].note_clips.push(d);
        assert!(!sync_links(&before, &mut after));
        assert_eq!(after.tracks[0].note_clips[0].notes.len(), 1);
    }

    #[test]
    fn switching_a_tracks_version_edits_nothing_else() {
        let mut before = song();
        // A linked copy on another track.
        let mut other = Track::new("o".into(), "Keys 2".into());
        other.kind = TrackKind::Instrument;
        let mut e = before.tracks[0].note_clips[0].clone();
        e.id = "e".into();
        other.note_clips.push(e);
        before.tracks.push(other);
        before.tracks[0].version = Some("first".into());
        let mut after = before.clone();
        after.tracks[0].version = Some("second".into());
        after.tracks[0].note_clips[0].notes.clear();
        assert!(!sync_links(&before, &mut after));
        assert_eq!(after.tracks[2].note_clips[0].notes.len(), 1);
    }

    #[test]
    fn groups_are_found_and_unlinking_breaks_one_off() {
        let mut p = song();
        let g = link_groups(&p);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].clips, vec!["a", "b"]);
        assert!(g[0].notes);
        assert_eq!(g[1].clips, vec!["x", "y"]);
        assert_eq!(instances(&p, "b"), 2);
        assert_eq!(instances(&p, "c"), 1);
        // Unlinking one of two leaves neither linked.
        assert!(unlink(&mut p, "a"));
        assert_eq!(p.tracks[0].note_clips[0].link, None);
        assert_eq!(p.tracks[0].note_clips[1].link, None);
        assert!(!unlink(&mut p, "a"));
        assert_eq!(link_groups(&p).len(), 1);
        // A clip of its own gets a link for a copy to share.
        let l = link_of(&mut p, "c").unwrap();
        assert_eq!(p.tracks[0].note_clips[2].link.as_deref(), Some(l.as_str()));
    }

    #[test]
    fn links_are_saved_and_older_projects_have_none() {
        let p = song();
        let mut doc = crate::ProjectDoc::new(1);
        doc.set(&p);
        let back = crate::ProjectDoc::from_bytes(&doc.to_bytes(), 2)
            .unwrap()
            .get();
        assert_eq!(back.tracks[0].note_clips[0].link.as_deref(), Some("L"));
        assert_eq!(back.tracks[0].note_clips[2].link, None);
        assert_eq!(back.tracks[1].clips[1].link.as_deref(), Some("A"));
    }
}
