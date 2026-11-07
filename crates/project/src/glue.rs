// SPDX-License-Identifier: GPL-3.0-or-later
//! Gluing note clips: several clips on one track become one clip that
//! plays what they played, so it moves, loops and is copied as one
//! piece. (Audio clips are glued by rendering them, in the app.)

use std::collections::HashMap;

use crate::{Lane, NoteClip};

/// Which clips of a track's `clips` (by id) can be glued together, in
/// time order, or why not. Two at least, none muted or playing a
/// pattern, and none of the others on the track lying wholly between
/// the first one's start and the last one's end (glue those too, or
/// move them). The names in the reasons are the clips' own.
pub fn glue_order<C>(
    clips: &[C],
    ids: &[&str],
    id: impl Fn(&C) -> &str,
    span: impl Fn(&C) -> (i64, i64),
    muted: impl Fn(&C) -> bool,
    name: impl Fn(&C) -> String,
) -> Result<Vec<usize>, String> {
    let mut picked: Vec<usize> = Vec::new();
    for want in ids {
        let i = (clips.iter().position(|c| id(c) == *want))
            .ok_or_else(|| format!("There is no clip {want} on that track."))?;
        if !picked.contains(&i) {
            picked.push(i);
        }
    }
    if picked.len() < 2 {
        return Err("Pick two clips or more on one track to glue.".into());
    }
    if let Some(&i) = picked.iter().find(|&&i| muted(&clips[i])) {
        return Err(format!(
            "{} is muted: unmute it, or leave it out.",
            name(&clips[i])
        ));
    }
    picked.sort_by_key(|&i| span(&clips[i]));
    let from = span(&clips[picked[0]]).0;
    let to = (picked.iter())
        .map(|&i| span(&clips[i]).1)
        .max()
        .unwrap_or(from);
    let between = (0..clips.len()).find(|i| {
        let (a, b) = span(&clips[*i]);
        !picked.contains(i) && !muted(&clips[*i]) && a >= from && b <= to
    });
    if let Some(i) = between {
        return Err(format!(
            "{} sits between them: glue it too, or move it out of the way.",
            name(&clips[i])
        ));
    }
    Ok(picked)
}

/// Glues the note clips `ids` of a track's `clips` into one, which
/// takes the first one's place: it runs from the first one's start to
/// the last one's end and plays the notes they played, where they
/// played them (each time round of a looping clip, each cut at its
/// clip's end), with their controller lanes. It keeps the first one's
/// name and key, doesn't loop and isn't linked. Gives its id.
pub fn glue_note_clips(clips: &mut Vec<NoteClip>, ids: &[&str]) -> Result<String, String> {
    let order = glue_order(
        clips,
        ids,
        |c| c.id.as_str(),
        |c| (c.start, c.end()),
        |c| c.muted,
        |c| format!("\"{}\"", c.name),
    )?;
    if let Some(&i) = order.iter().find(|&&i| clips[i].pattern.is_some()) {
        return Err(format!(
            "\"{}\" plays a drum pattern: convert it to plain notes first.",
            clips[i].name
        ));
    }
    let first = &clips[order[0]];
    let start = first.start;
    let end = (order.iter())
        .map(|&i| clips[i].end())
        .max()
        .unwrap_or(start);
    let mut glued = NoteClip::new(crate::new_id(), first.name.clone(), start, end - start);
    glued.scale = first.scale;
    // Chance groups are a clip's own, and rolled each time round: kept
    // apart by numbering them afresh.
    let mut groups: HashMap<(usize, u32, u32), u32> = HashMap::new();
    for (k, &i) in order.iter().enumerate() {
        let c = &clips[i];
        let cut = c.end();
        for (round, n) in c.sounding() {
            let mut n = n;
            n.len = n.len.min(cut - n.start).max(1);
            n.start -= start;
            n.id = crate::new_id();
            if let Some(g) = n.group {
                let next = groups.len() as u32 + 1;
                n.group = Some(*groups.entry((k, round, g)).or_insert(next));
            }
            glued.notes.push(n);
        }
    }
    glued.sort_notes();
    // Controller lanes, each time round, joined per controller: a
    // clip's value holds through any gap until the next one's starts.
    let mut lanes: Vec<Lane> = Vec::new();
    for &i in &order {
        let c = &clips[i];
        for l in c.controllers.iter().filter(|l| !l.points.is_empty()) {
            let at = match lanes.iter().position(|x| x.param == l.param) {
                Some(at) => at,
                None => {
                    lanes.push(Lane::new(l.param.clone()));
                    lanes.len() - 1
                }
            };
            let out = &mut lanes[at];
            for (a, b, shift) in c.spans() {
                let rel = |t: i64| c.place(t + shift) - start;
                let (Some(v0), Some(v1)) = (l.value_at(a), l.value_at(b - 1)) else {
                    continue;
                };
                if let Some(last) = out.points.last().copied()
                    && last.at < rel(a) - 1
                {
                    out.put(rel(a) - 1, last.value);
                }
                out.put(rel(a), v0);
                for p in l.points.iter().filter(|p| p.at > a && p.at < b) {
                    out.put(rel(p.at), p.value);
                }
                if l.points.iter().any(|p| p.at >= b) {
                    out.put(rel(b - 1), v1);
                }
            }
        }
    }
    glued.controllers = lanes;
    let id = glued.id.clone();
    let at = order[0];
    let mut gone = order.clone();
    gone.sort_unstable();
    clips[at] = glued;
    for i in gone.into_iter().rev().filter(|&i| i != at) {
        clips.remove(i);
    }
    clips.sort_by_key(|c| c.start);
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Controller, Note, PPQ};

    const BAR: i64 = PPQ * 4;

    fn clip(id: &str, start: i64, len: i64, notes: &[(i64, i64, u8)]) -> NoteClip {
        let mut c = NoteClip::new(id.into(), id.to_uppercase(), start, len);
        c.notes = (notes.iter())
            .map(|&(s, l, p)| Note::new(s, l, p, 100))
            .collect();
        c
    }

    fn placed(clips: &[NoteClip]) -> Vec<(i64, i64, u8)> {
        let mut out: Vec<(i64, i64, u8)> = (clips.iter())
            .filter(|c| !c.muted)
            .flat_map(|c| c.placed())
            .map(|n| (n.start, n.len, n.pitch))
            .collect();
        out.sort();
        out
    }

    #[test]
    fn glued_clips_play_what_they_played() {
        // A trimmed clip, a looping one after a gap, and one running
        // a note past its end.
        let mut a = clip("a", BAR, BAR, &[(0, PPQ, 60), (PPQ * 5, PPQ, 61)]);
        a.offset = PPQ;
        let mut b = clip(
            "b",
            BAR * 3,
            BAR * 2,
            &[(0, PPQ, 64), (PPQ * 2, PPQ * 4, 65)],
        );
        b.loop_len = Some(BAR);
        let c = clip("c", BAR * 5, BAR, &[(PPQ * 3, PPQ * 3, 67)]);
        let other = clip("x", BAR * 8, BAR, &[(0, PPQ, 50)]);
        let mut clips = vec![c, other, b, a];
        let before = placed(&clips);
        let id = glue_note_clips(&mut clips, &["c", "a", "b"]).unwrap();
        assert_eq!(clips.len(), 2);
        let g = &clips[0];
        assert_eq!(g.id, id);
        assert_eq!((g.start, g.len, g.offset), (BAR, BAR * 5, 0));
        assert_eq!(g.name, "A", "the first one's name");
        assert_eq!((g.loop_len, &g.link, &g.take), (None, &None, &None));
        assert_eq!(clips[1].id, "x", "the others are left alone");
        assert_eq!(placed(&clips), before);
    }

    #[test]
    fn only_clips_that_can_be_glued_are() {
        let a = clip("a", 0, BAR, &[]);
        let b = clip("b", BAR, BAR, &[]);
        let c = clip("c", BAR * 2, BAR, &[]);
        let mut clips = vec![a, b, c];
        let e = glue_note_clips(&mut clips, &["a"]).unwrap_err();
        assert!(e.contains("two clips"), "{e}");
        let e = glue_note_clips(&mut clips, &["a", "nope"]).unwrap_err();
        assert!(e.contains("no clip nope"), "{e}");
        let e = glue_note_clips(&mut clips, &["a", "c"]).unwrap_err();
        assert!(e.contains("\"B\" sits between"), "{e}");
        clips[1].muted = true;
        let e = glue_note_clips(&mut clips, &["a", "b"]).unwrap_err();
        assert!(e.contains("\"B\" is muted"), "{e}");
        // A muted clip between is left where it is.
        glue_note_clips(&mut clips, &["a", "c"]).unwrap();
        assert_eq!(clips.len(), 2);
        clips[0].pattern = Some("p".into());
        clips[1].muted = false;
        let ids = [clips[0].id.clone(), clips[1].id.clone()];
        let e = glue_note_clips(&mut clips, &[&ids[0], &ids[1]]).unwrap_err();
        assert!(e.contains("drum pattern"), "{e}");
    }

    #[test]
    fn controller_lanes_follow_the_notes() {
        let mut a = clip("a", 0, BAR, &[(0, PPQ, 60)]);
        let l = a.controller_mut(Controller::Cc(1));
        l.put(0, 0.0);
        l.put(BAR, 127.0);
        let mut b = clip("b", BAR * 2, BAR, &[(0, PPQ, 62)]);
        b.controller_mut(Controller::Cc(1)).put(0, 20.0);
        let mut clips = vec![a, b];
        glue_note_clips(&mut clips, &["a", "b"]).unwrap();
        let l = &clips[0].controllers[0];
        assert_eq!(l.param, "cc:1");
        // The ramp up to the first clip's end, held through the gap,
        // then the second clip's value.
        assert_eq!(l.value_at(BAR / 2), Some(63.5));
        let held = l.value_at(BAR - 1).unwrap();
        assert!((held - 127.0).abs() < 0.1, "{held}");
        assert_eq!(l.value_at(BAR + BAR / 2), Some(held));
        assert_eq!(l.value_at(BAR * 2), Some(20.0));
        assert_eq!(l.value_at(BAR * 3), Some(20.0));
    }

    #[test]
    fn chance_groups_stay_apart() {
        let mut a = clip("a", 0, BAR, &[(0, PPQ, 60), (PPQ, PPQ, 62)]);
        a.notes[0].group = Some(1);
        a.notes[1].group = Some(1);
        let mut b = clip("b", BAR, BAR, &[(0, PPQ, 64)]);
        b.notes[0].group = Some(1);
        let mut clips = vec![a, b];
        glue_note_clips(&mut clips, &["a", "b"]).unwrap();
        let groups: Vec<Option<u32>> = clips[0].notes.iter().map(|n| n.group).collect();
        assert_eq!(groups, [Some(1), Some(1), Some(2)]);
    }
}
