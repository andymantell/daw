// SPDX-License-Identifier: GPL-3.0-or-later
//! Inserting and deleting whole bars across the whole song: every
//! track's clips and notes, automation, tempo and time-signature
//! changes, markers, the loop, assignments, comments, hit points and
//! video move together, so nothing falls out of step.
//!
//! Positions are kept in three units (bars for markers and tempo
//! changes, ticks for notes, frames for audio), and this crate has no
//! tempo map, so the caller says where the bars fall in each: see
//! [`Bars`].
//!
//! Inserted bars are empty, and take the time signature and tempo in
//! force just before them. A clip across the point is cut in two, its
//! second part moved on; a clip across deleted bars loses the part
//! inside them. The cuts are clean, as a split's are: fades stay at the
//! outer ends. A video and its sound move whole, never cut, so the
//! picture stays with its sound.

use std::collections::HashSet;

use crate::{AutoPoint, Clip, Fade, Lane, NoteClip, Project, new_id};

/// A run of whole bars, and where it falls in ticks and frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bars {
    /// The first bar, counted from 1.
    pub bar: i64,
    /// How many bars; at least 1.
    pub count: i64,
    /// Where the first bar starts, and how long the bars last, in ticks
    /// (for inserting, as long as they will be).
    pub tick: i64,
    pub ticks: i64,
    /// The same in frames at the project's rate.
    pub frame: i64,
    pub frames: i64,
}

/// Puts `b.count` empty bars before bar `b.bar`: everything from there
/// on moves later.
pub fn insert_bars(p: &mut Project, b: &Bars) {
    if b.count < 1 {
        return;
    }
    let (bar, n) = (b.bar.max(1), b.count);
    let later = |x: i64| if x >= bar { x + n } else { x };
    for m in &mut p.markers {
        m.bar = later(m.bar);
    }
    for c in &mut p.tempo_changes {
        c.bar = later(c.bar);
    }
    for c in &mut p.meter_changes {
        c.bar = later(c.bar);
    }
    // A loop or assignment over the point grows; one after it moves.
    let span = |(a, z): (i64, i64)| (later(a), if z > bar { z + n } else { z });
    p.loop_bars = span(p.loop_bars);
    for a in &mut p.assignments {
        a.bars = span(a.bars);
    }

    let (f, l) = (b.frame, b.frames);
    let fl = |x: i64| if x >= f { x + l } else { x };
    let whole = whole_clips(p);
    for t in &mut p.tracks {
        let mut clips = Vec::with_capacity(t.clips.len());
        for mut c in std::mem::take(&mut t.clips) {
            if whole.contains(&c.id) {
                c.start = fl(c.start);
                clips.push(c);
            } else if c.start < f && f < c.start + c.len {
                let (a, mut z) = cut_clip(c, f);
                z.start += l;
                clips.extend([a, z]);
            } else {
                c.start = fl(c.start);
                clips.push(c);
            }
        }
        t.clips = clips;

        let mut notes = Vec::with_capacity(t.note_clips.len());
        for mut c in std::mem::take(&mut t.note_clips) {
            if c.start < b.tick && b.tick < c.end() {
                let (a, mut z) = cut_notes(c, b.tick);
                z.start += b.ticks;
                notes.extend([a, z]);
            } else {
                if c.start >= b.tick {
                    c.start += b.ticks;
                }
                notes.push(c);
            }
        }
        t.note_clips = notes;

        for comp in &mut t.comps {
            let mut parts = Vec::with_capacity(comp.parts.len());
            for mut part in std::mem::take(&mut comp.parts) {
                if part.start < f && f < part.end {
                    parts.push(crate::CompPart { end: f, ..part });
                    part.start = f + l;
                    part.end += l;
                } else {
                    part.start = fl(part.start);
                    part.end = fl(part.end);
                }
                parts.push(part);
            }
            comp.parts = parts;
        }
        for lane in &mut t.automation {
            insert_in_lane(lane, f, l);
        }
    }
    for v in &mut p.videos {
        v.start = fl(v.start);
    }
    for c in &mut p.comments {
        c.start = fl(c.start);
        c.end = c.end.map(|e| if e > f { e + l } else { e });
    }
    for h in &mut p.hit_points {
        h.at = fl(h.at);
    }
    p.timecode.offset = fl(p.timecode.offset);
}

/// Takes bars `b.bar` to `b.bar + b.count` out: what was in them goes,
/// and everything after moves earlier. The tempo and time signature
/// after the bars stay as they were.
pub fn delete_bars(p: &mut Project, b: &Bars) {
    if b.count < 1 {
        return;
    }
    let (bar, n) = (b.bar.max(1), b.count);
    let end = bar + n;
    let inside = |x: i64| x >= bar && x < end;
    let earlier = |x: i64| {
        if x >= end {
            x - n
        } else if x > bar {
            bar
        } else {
            x
        }
    };
    p.markers.retain(|m| !inside(m.bar));
    for m in &mut p.markers {
        m.bar = earlier(m.bar);
    }

    // The last change inside the bars is what played after them, so it
    // moves to where they began.
    let last_tempo = (p.tempo_changes.iter())
        .filter(|c| inside(c.bar))
        .max_by_key(|c| (c.bar, c.beat))
        .cloned();
    p.tempo_changes.retain(|c| !inside(c.bar));
    for c in &mut p.tempo_changes {
        c.bar -= if c.bar >= end { n } else { 0 };
    }
    if let Some(mut c) = last_tempo {
        if bar == 1 {
            p.tempo_bpm = c.bpm;
        } else {
            (c.bar, c.beat, c.glide) = (bar, 1, false);
            p.tempo_changes.push(c);
        }
    }
    let last_meter = (p.meter_changes.iter())
        .filter(|c| inside(c.bar))
        .max_by_key(|c| c.bar)
        .cloned();
    p.meter_changes.retain(|c| !inside(c.bar));
    for c in &mut p.meter_changes {
        c.bar -= if c.bar >= end { n } else { 0 };
    }
    if let Some(mut c) = last_meter {
        if bar == 1 {
            (p.beats_per_bar, p.beat_unit) = (c.beats, c.unit);
        } else {
            c.bar = bar;
            p.meter_changes.push(c);
        }
    }

    let span = |(a, z): (i64, i64)| {
        let a = earlier(a);
        (a, earlier(z).max(a + 1))
    };
    p.loop_bars = span(p.loop_bars);
    for a in &mut p.assignments {
        a.bars = span(a.bars);
    }

    let (f, l) = (b.frame, b.frames);
    let e = f + l;
    let fl = |x: i64| {
        if x >= e {
            x - l
        } else if x > f {
            f
        } else {
            x
        }
    };
    let whole = whole_clips(p);
    let gone: HashSet<String> = (p.videos.iter())
        .filter(|v| v.start >= f && v.start + v.len <= e)
        .flat_map(|v| v.sound.clone())
        .collect();
    p.videos.retain(|v| !(v.start >= f && v.start + v.len <= e));
    for v in &mut p.videos {
        v.start = fl(v.start);
    }
    for t in &mut p.tracks {
        let mut clips = Vec::with_capacity(t.clips.len());
        for mut c in std::mem::take(&mut t.clips) {
            let end_at = c.start + c.len;
            if gone.contains(&c.id) {
            } else if whole.contains(&c.id) {
                c.start = fl(c.start);
                clips.push(c);
            } else if end_at <= f {
                clips.push(c);
            } else if c.start >= e {
                c.start -= l;
                clips.push(c);
            } else if c.start >= f && end_at <= e {
                // All inside: gone.
            } else if c.start < f && end_at > e {
                let (a, rest) = cut_clip(c, f);
                let (_, mut z) = cut_clip(rest, e);
                z.start = f;
                clips.extend([a, z]);
            } else if c.start < f {
                clips.push(cut_clip(c, f).0);
            } else {
                let mut z = cut_clip(c, e).1;
                z.start = f;
                clips.push(z);
            }
        }
        t.clips = clips;

        let (tf, tl) = (b.tick, b.ticks);
        let te = tf + tl;
        let mut notes = Vec::with_capacity(t.note_clips.len());
        for mut c in std::mem::take(&mut t.note_clips) {
            if c.end() <= tf {
                notes.push(c);
            } else if c.start >= te {
                c.start -= tl;
                notes.push(c);
            } else if c.start >= tf && c.end() <= te {
            } else if c.start < tf && c.end() > te {
                let (a, rest) = cut_notes(c, tf);
                let (_, mut z) = cut_notes(rest, te);
                z.start = tf;
                notes.extend([a, z]);
            } else if c.start < tf {
                notes.push(cut_notes(c, tf).0);
            } else {
                let mut z = cut_notes(c, te).1;
                z.start = tf;
                notes.push(z);
            }
        }
        t.note_clips = notes;

        for comp in &mut t.comps {
            comp.parts.retain(|q| !(q.start >= f && q.end <= e));
            for q in &mut comp.parts {
                (q.start, q.end) = (fl(q.start), fl(q.end));
            }
            comp.parts.retain(|q| q.end > q.start);
        }
        for lane in &mut t.automation {
            delete_in_lane(lane, f, l);
        }
    }
    // Comments are people's words: they move to the cut, never go.
    for c in &mut p.comments {
        c.start = fl(c.start);
        c.end = c.end.map(fl);
    }
    p.hit_points.retain(|h| !(h.at >= f && h.at < e));
    for h in &mut p.hit_points {
        h.at = fl(h.at);
    }
    p.timecode.offset = fl(p.timecode.offset);
}

/// Clips that move whole: the sound of a video.
fn whole_clips(p: &Project) -> HashSet<String> {
    p.videos.iter().flat_map(|v| v.sound.clone()).collect()
}

/// Cuts an audio clip in two at timeline frame `at`, inside it. The
/// second part is new; the fades stay at the outer ends.
fn cut_clip(c: Clip, at: i64) -> (Clip, Clip) {
    let first = at - c.start;
    let mut a = c.clone();
    a.len = first;
    a.fade_out = Fade::default();
    a.fade_in.len = a.fade_in.len.min(a.len);
    let mut z = c;
    z.id = new_id();
    z.start = at;
    z.offset += first;
    z.len -= first;
    z.fade_in = Fade::default();
    z.fade_out.len = z.fade_out.len.min(z.len);
    (a, z)
}

/// Cuts a note clip in two at tick `at`, inside it. Both keep the
/// content (each plays the notes that begin inside it); the second part
/// and its notes get new ids.
fn cut_notes(c: NoteClip, at: i64) -> (NoteClip, NoteClip) {
    let first = at - c.start;
    let mut a = c.clone();
    a.len = first;
    let mut z = c;
    z.id = new_id();
    for n in &mut z.notes {
        n.id = new_id();
    }
    z.start = at;
    z.offset += first;
    z.len -= first;
    (a, z)
}

/// Opens `len` frames at `at` in a lane, holding its value there.
fn insert_in_lane(lane: &mut Lane, at: i64, len: i64) {
    let Some(v) = lane.value_at(at) else {
        return;
    };
    let before = lane.points.iter().any(|q| q.at < at);
    let after = lane.points.iter().any(|q| q.at >= at);
    for q in &mut lane.points {
        if q.at >= at {
            q.at += len;
        }
    }
    if before && after {
        lane.put(at, v);
        lane.put(at + len, v);
    }
}

/// Takes `len` frames at `at` out of a lane: the value up to the cut,
/// and from it, stay as they were.
fn delete_in_lane(lane: &mut Lane, at: i64, len: i64) {
    if lane.points.iter().all(|q| q.at < at) {
        return;
    }
    let end = at + len;
    let (Some(v0), Some(v1)) = (lane.value_at(at), lane.value_at(end)) else {
        return;
    };
    let mut points: Vec<AutoPoint> = (lane.points.iter())
        .filter(|q| q.at < at || q.at >= end)
        .map(|q| AutoPoint {
            at: if q.at >= end { q.at - len } else { q.at },
            value: q.value,
        })
        .collect();
    points.sort_by_key(|q| q.at);
    lane.points = points;
    if v0 != v1 && at >= 1 {
        lane.put(at - 1, v0);
    }
    lane.put(at, v1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Assignment, Comment, Comp, CompPart, Curve, HitPoint, Marker, MeterChange, Note,
        TempoChange, Track, VideoClip,
    };

    /// 4/4 at 120 bpm and 48 kHz: a bar is 2 s.
    const BAR: i64 = 96_000;
    const TBAR: i64 = crate::PPQ * 4;

    fn bars(bar: i64, count: i64) -> Bars {
        Bars {
            bar,
            count,
            tick: (bar - 1) * TBAR,
            ticks: count * TBAR,
            frame: (bar - 1) * BAR,
            frames: count * BAR,
        }
    }

    fn clip(id: &str, start: i64, len: i64) -> Clip {
        Clip {
            id: id.into(),
            asset: "a.wav".into(),
            start,
            len,
            fade_in: Fade {
                len: 1000,
                curve: Curve::Linear,
            },
            fade_out: Fade {
                len: 1000,
                curve: Curve::Linear,
            },
            ..Clip::default()
        }
    }

    fn marker(id: &str, bar: i64) -> Marker {
        Marker {
            id: id.into(),
            bar,
            name: id.into(),
        }
    }

    fn song() -> Project {
        let mut t = Track::new("t".into(), "Guitar".into());
        // Bars 1–2, bars 2–4 and bars 5–6.
        t.clips = vec![
            clip("a", 0, 2 * BAR),
            clip("b", BAR, 3 * BAR),
            clip("c", 4 * BAR, 2 * BAR),
        ];
        let mut nc = NoteClip::new("n".into(), "Riff".into(), 0, 4 * TBAR);
        nc.notes = vec![
            Note::new(0, 960, 60, 100),
            Note::new(2 * TBAR, 960, 64, 100),
        ];
        t.note_clips = vec![nc];
        let mut lane = Lane::new(Lane::GAIN);
        lane.put(0, 0.0);
        lane.put(4 * BAR, -12.0);
        t.automation = vec![lane];
        Project {
            tracks: vec![t],
            markers: vec![marker("verse", 1), marker("chorus", 3), marker("solo", 5)],
            tempo_changes: vec![TempoChange {
                id: "tc".into(),
                bar: 5,
                beat: 1,
                bpm: 90.0,
                glide: false,
            }],
            loop_bars: (3, 5),
            ..Project::default()
        }
    }

    fn starts(p: &Project) -> Vec<(String, i64, i64, i64)> {
        let mut v: Vec<_> = (p.tracks[0].clips.iter())
            .map(|c| (c.id.clone(), c.start, c.offset, c.len))
            .collect();
        v.sort_by_key(|c| (c.1, c.0.clone()));
        v
    }

    #[test]
    fn inserting_bars_moves_everything_after_and_cuts_what_is_across() {
        let mut p = song();
        insert_bars(&mut p, &bars(3, 2));
        let m: Vec<i64> = p.markers.iter().map(|m| m.bar).collect();
        assert_eq!(m, [1, 5, 7]);
        assert_eq!(p.tempo_changes[0].bar, 7);
        assert_eq!(p.loop_bars, (5, 7));
        let c = starts(&p);
        // "a" ends at bar 3 and stays; "b" is cut at bar 3, its second
        // part two bars on; "c" moves.
        assert_eq!(c[0], ("a".into(), 0, 0, 2 * BAR));
        assert_eq!(c[1], ("b".into(), BAR, 0, BAR));
        assert_eq!((c[2].1, c[2].2, c[2].3), (4 * BAR, BAR, 2 * BAR));
        assert_eq!(c[3], ("c".into(), 6 * BAR, 0, 2 * BAR));
        // The fades stay at the outer ends.
        let b2 = p.tracks[0]
            .clips
            .iter()
            .find(|x| x.start == 4 * BAR)
            .unwrap();
        assert_eq!((b2.fade_in.len, b2.fade_out.len), (0, 1000));
        let b1 = p.tracks[0].clips.iter().find(|x| x.id == "b").unwrap();
        assert_eq!((b1.fade_in.len, b1.fade_out.len), (1000, 0));
        // The note clip is cut; each part plays the same notes as before.
        let nc = &p.tracks[0].note_clips;
        assert_eq!(nc.len(), 2);
        let played: Vec<(i64, u8)> = nc
            .iter()
            .flat_map(|c| c.placed())
            .map(|n| (n.start, n.pitch))
            .collect();
        assert_eq!(played, [(0, 60), (4 * TBAR, 64)]);
        // The gain line holds over the new bars.
        let lane = &p.tracks[0].automation[0];
        assert_eq!(lane.value_at(2 * BAR), Some(-6.0));
        assert_eq!(lane.value_at(3 * BAR), Some(-6.0));
        assert_eq!(lane.value_at(4 * BAR), Some(-6.0));
        assert_eq!(lane.value_at(6 * BAR), Some(-12.0));
    }

    #[test]
    fn deleting_bars_takes_out_what_is_in_them_and_closes_the_gap() {
        let mut p = song();
        delete_bars(&mut p, &bars(3, 2));
        let m: Vec<(&str, i64)> = p.markers.iter().map(|m| (m.name.as_str(), m.bar)).collect();
        assert_eq!(m, [("verse", 1), ("solo", 3)]);
        assert_eq!(p.tempo_changes[0].bar, 3);
        // The loop was the deleted bars: it keeps a bar where they were.
        assert_eq!(p.loop_bars, (3, 4));
        let c = starts(&p);
        // "b" loses bars 3–4, keeping bar 2; "c" moves back two bars.
        assert_eq!(
            c,
            [
                ("a".into(), 0, 0, 2 * BAR),
                ("b".into(), BAR, 0, BAR),
                ("c".into(), 2 * BAR, 0, 2 * BAR),
            ]
        );
        // The note clip loses its second half, and the note in it.
        let nc = &p.tracks[0].note_clips;
        assert_eq!(nc.len(), 1);
        assert_eq!(nc[0].placed().count(), 1);
        // The line keeps its values either side of the cut.
        let lane = &p.tracks[0].automation[0];
        assert_eq!(lane.value_at(0), Some(0.0));
        assert_eq!(lane.value_at(2 * BAR - 1), Some(-6.0));
        assert_eq!(lane.value_at(2 * BAR), Some(-12.0));
    }

    #[test]
    fn the_tempo_and_time_signature_after_deleted_bars_stay() {
        let mut p = song();
        p.tempo_changes[0].bar = 2;
        p.tempo_changes[0].beat = 3;
        p.meter_changes = vec![MeterChange {
            id: "m".into(),
            bar: 2,
            beats: 3,
            unit: 4,
        }];
        let mut q = p.clone();
        delete_bars(&mut p, &bars(2, 2));
        assert_eq!((p.tempo_changes[0].bar, p.tempo_changes[0].beat), (2, 1));
        assert_eq!(p.meter_changes[0].bar, 2);
        // From the very start, the song's own tempo and signature change.
        delete_bars(&mut q, &bars(1, 2));
        assert!(q.tempo_changes.is_empty() && q.meter_changes.is_empty());
        assert_eq!((q.tempo_bpm, q.beats_per_bar), (90.0, 3));
    }

    #[test]
    fn comments_hit_points_video_and_comps_move_too() {
        let mut p = song();
        p.comments = vec![Comment {
            id: "k".into(),
            author: String::new(),
            parent: None,
            track: None,
            start: 3 * BAR,
            end: Some(5 * BAR),
            text: "Nice".into(),
            resolved: false,
            created: 0,
        }];
        p.hit_points = vec![
            HitPoint {
                id: "h1".into(),
                at: 3 * BAR,
                name: "Door".into(),
            },
            HitPoint {
                id: "h2".into(),
                at: 5 * BAR,
                name: "Car".into(),
            },
        ];
        p.videos = vec![VideoClip {
            id: "v".into(),
            start: BAR,
            len: 4 * BAR,
            sound: Some("b".into()),
            ..VideoClip::default()
        }];
        p.assignments = vec![Assignment {
            id: "as".into(),
            bars: (5, 7),
            bpm: 90.0,
            due: 0,
            note: String::new(),
            lesson: None,
            author: String::new(),
            created: 0,
        }];
        p.tracks[0].comps = vec![Comp {
            id: "cp".into(),
            name: "Comp 1".into(),
            parts: vec![CompPart {
                take: 0,
                start: BAR,
                end: 4 * BAR,
            }],
        }];
        let mut q = p.clone();
        insert_bars(&mut p, &bars(3, 1));
        assert_eq!(
            (p.comments[0].start, p.comments[0].end),
            (4 * BAR, Some(6 * BAR))
        );
        assert_eq!(p.hit_points[0].at, 4 * BAR);
        // The video starts before the point: it and its sound stay whole.
        assert_eq!(p.videos[0].start, BAR);
        let b = p.tracks[0].clips.iter().find(|c| c.id == "b").unwrap();
        assert_eq!((b.start, b.len), (BAR, 3 * BAR));
        assert_eq!(p.assignments[0].bars, (6, 8));
        let parts = &p.tracks[0].comps[0].parts;
        assert_eq!(
            parts.iter().map(|q| (q.start, q.end)).collect::<Vec<_>>(),
            [(BAR, 2 * BAR), (3 * BAR, 5 * BAR)]
        );

        delete_bars(&mut q, &bars(3, 2));
        // A comment inside moves to the cut; a hit point inside goes.
        assert_eq!(
            (q.comments[0].start, q.comments[0].end),
            (2 * BAR, Some(3 * BAR))
        );
        assert_eq!(q.hit_points.len(), 1);
        assert_eq!(q.hit_points[0].at, 3 * BAR);
        assert_eq!(q.assignments[0].bars, (3, 5));
        let parts = &q.tracks[0].comps[0].parts;
        assert_eq!(
            parts.iter().map(|q| (q.start, q.end)).collect::<Vec<_>>(),
            [(BAR, 2 * BAR)]
        );
    }
}
