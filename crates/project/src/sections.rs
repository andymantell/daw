// SPDX-License-Identifier: GPL-3.0-or-later
//! Song sections (Phase 4, A4): the bars from one marker to the next
//! (the last runs to the end of the song). A section can be copied out
//! of the song as a piece, pasted back in anywhere, and a play order of
//! sections, with repeats, written out as a plain timeline.
//!
//! Like [`crate::insert_bars`], these need to know where bars fall in
//! ticks and frames, which this crate can't work out: the caller passes
//! a [`Clock`] for the song as it is.
//!
//! Video, its sound and hit points belong to the picture, not to a
//! section: they are left where they are.

use std::collections::{HashMap, HashSet};

use crate::{Bars, MeterChange, Project, TempoChange, delete_bars, insert_bars, new_id};

/// Where a bar (counted from 1) starts, in ticks and in frames, in the
/// song as it was before the edit.
pub type Clock<'a> = &'a dyn Fn(i64) -> (i64, i64);

/// Bars `from` to `to`, by the clock.
pub fn bars_between(clock: Clock, from: i64, to: i64) -> Bars {
    let (t0, f0) = clock(from);
    let (t1, f1) = clock(to);
    Bars {
        bar: from,
        count: to - from,
        tick: t0,
        ticks: t1 - t0,
        frame: f0,
        frames: f1 - f0,
    }
}

/// A section of the song: from its marker's bar to the next marker's,
/// or to the song's end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    /// Its marker's id.
    pub id: String,
    pub name: String,
    /// Bars, counted from 1: from the start of the first to the start of
    /// the second.
    pub from: i64,
    pub to: i64,
}

/// The song's sections, in bar order. `end` is the bar the song ends
/// at (the start of the first bar with nothing in it).
pub fn sections(p: &Project, end: i64) -> Vec<Section> {
    let mut m: Vec<_> = p.markers.iter().collect();
    m.sort_by_key(|m| m.bar);
    (m.iter().enumerate())
        .map(|(i, mk)| {
            let to = match m.get(i + 1) {
                Some(n) => n.bar,
                None => end.max(mk.bar + 1),
            };
            Section {
                id: mk.id.clone(),
                name: mk.name.clone(),
                from: mk.bar,
                to,
            }
        })
        .collect()
}

/// Bars `from` to `to` of the song, on their own, starting at bar 1:
/// everything else taken out. `end` is where the song ends. With
/// `comments`, the comments pinned inside come too. Video, its sound
/// and hit points never do.
pub fn piece(p: &Project, clock: Clock, from: i64, to: i64, end: i64, comments: bool) -> Project {
    let mut q = p.clone();
    let sounds: HashSet<String> = p.videos.iter().flat_map(|v| v.sound.clone()).collect();
    q.videos.clear();
    q.hit_points.clear();
    for t in &mut q.tracks {
        t.clips.retain(|c| !sounds.contains(&c.id));
    }
    let (_, f0) = clock(from);
    let (_, f1) = clock(to);
    let kept: HashSet<String> = (q.comments.iter())
        .filter(|c| comments && c.parent.is_none() && c.start >= f0 && c.start < f1)
        .map(|c| c.id.clone())
        .collect();
    q.comments
        .retain(|c| kept.contains(c.parent.as_ref().unwrap_or(&c.id)));
    let end = end.max(to) + 1;
    delete_bars(&mut q, &bars_between(clock, to, end));
    if from > 1 {
        delete_bars(&mut q, &bars_between(clock, 1, from));
    }
    q
}

/// The tempo in force at the start of `bar`, from changes before it.
fn tempo_before(p: &Project, bar: i64) -> f64 {
    (p.tempo_changes.iter())
        .filter(|c| c.bar < bar)
        .max_by_key(|c| (c.bar, c.beat))
        .map_or(p.tempo_bpm, |c| c.bpm)
}

/// The tempo in force on beat 1 of `bar`.
fn tempo_at(p: &Project, bar: i64) -> f64 {
    (p.tempo_changes.iter())
        .filter(|c| (c.bar, c.beat) <= (bar, 1))
        .max_by_key(|c| (c.bar, c.beat))
        .map_or(p.tempo_bpm, |c| c.bpm)
}

/// The time signature in force from `bar`.
fn meter_at(p: &Project, bar: i64) -> (u32, u32) {
    (p.meter_changes.iter())
        .filter(|c| c.bar <= bar)
        .max_by_key(|c| c.bar)
        .map_or((p.beats_per_bar, p.beat_unit), |c| (c.beats, c.unit))
}

fn set_tempo(p: &mut Project, bar: i64, bpm: f64) {
    p.tempo_changes.retain(|c| !(c.bar == bar && c.beat == 1));
    if bar <= 1 {
        p.tempo_bpm = bpm;
    } else {
        p.tempo_changes.push(TempoChange {
            id: new_id(),
            bar,
            beat: 1,
            bpm,
            glide: false,
        });
    }
}

fn set_meter(p: &mut Project, bar: i64, (beats, unit): (u32, u32)) {
    p.meter_changes.retain(|c| c.bar != bar);
    if bar <= 1 {
        (p.beats_per_bar, p.beat_unit) = (beats, unit);
    } else {
        p.meter_changes.push(MeterChange {
            id: new_id(),
            bar,
            beats,
            unit,
        });
    }
}

/// Puts `piece` (made by [`piece`]) into the song before bar `at.bar`,
/// moving what was there later. `at` says where, and how long the piece
/// is: its bars, ticks and frames. With `fresh`, everything pasted gets
/// a new id, as a copy; without, it keeps the piece's.
pub fn paste_bars(p: &mut Project, piece: &Project, at: &Bars, fresh: bool) {
    let (b, n) = (at.bar.max(1), at.count);
    if n < 1 {
        return;
    }
    let id = |s: &str| if fresh { new_id() } else { s.to_string() };
    // What plays after the piece stays as it was.
    let (after_tempo, after_meter) = (tempo_at(p, b), meter_at(p, b));
    insert_bars(p, at);
    let piece_tempo = |bar: i64| tempo_at(piece, bar);
    if tempo_before(p, b) != piece.tempo_bpm || b == 1 {
        set_tempo(p, b, piece.tempo_bpm);
    }
    if meter_at(p, b - 1) != (piece.beats_per_bar, piece.beat_unit) || b == 1 {
        set_meter(p, b, (piece.beats_per_bar, piece.beat_unit));
    }
    for c in piece.tempo_changes.iter().filter(|c| c.bar <= n) {
        if (c.bar, c.beat) == (1, 1) {
            continue;
        }
        p.tempo_changes.push(TempoChange {
            id: id(&c.id),
            bar: c.bar + b - 1,
            ..c.clone()
        });
    }
    for c in piece
        .meter_changes
        .iter()
        .filter(|c| c.bar > 1 && c.bar <= n)
    {
        p.meter_changes.push(MeterChange {
            id: id(&c.id),
            bar: c.bar + b - 1,
            ..c.clone()
        });
    }
    let has_tempo = p
        .tempo_changes
        .iter()
        .any(|c| (c.bar, c.beat) == (b + n, 1));
    if !has_tempo && piece_tempo(n + 1) != after_tempo {
        set_tempo(p, b + n, after_tempo);
    }
    let has_meter = p.meter_changes.iter().any(|c| c.bar == b + n);
    if !has_meter && meter_at(piece, n) != after_meter {
        set_meter(p, b + n, after_meter);
    }
    p.tempo_changes.sort_by_key(|c| (c.bar, c.beat));
    p.meter_changes.sort_by_key(|c| c.bar);

    for m in piece.markers.iter().filter(|m| m.bar <= n) {
        p.markers.push(crate::Marker {
            id: id(&m.id),
            bar: m.bar + b - 1,
            name: m.name.clone(),
        });
    }
    p.markers.sort_by_key(|m| m.bar);

    for pt in &piece.tracks {
        let Some(t) = p.tracks.iter_mut().find(|t| t.id == pt.id) else {
            continue;
        };
        for c in &pt.clips {
            t.clips.push(crate::Clip {
                id: id(&c.id),
                start: c.start + at.frame,
                ..c.clone()
            });
        }
        for c in &pt.note_clips {
            let mut c = c.clone();
            c.id = id(&c.id);
            if fresh {
                for note in &mut c.notes {
                    note.id = new_id();
                }
            }
            c.start += at.tick;
            t.note_clips.push(c);
        }
        t.note_clips.sort_by_key(|c| c.start);
        for comp in &pt.comps {
            let Some(to) = t.comps.iter_mut().find(|c| c.id == comp.id) else {
                continue;
            };
            for q in &comp.parts {
                to.parts.push(crate::CompPart {
                    start: q.start + at.frame,
                    end: q.end + at.frame,
                    ..*q
                });
            }
            to.parts.sort_by_key(|q| q.start);
        }
        for lane in pt.automation.iter().filter(|l| !l.points.is_empty()) {
            let i = match t.automation.iter().position(|l| l.param == lane.param) {
                Some(i) => i,
                None => {
                    t.automation.push(crate::Lane::new(lane.param.clone()));
                    t.automation.len() - 1
                }
            };
            let to = &mut t.automation[i];
            for q in lane.points.iter().filter(|q| q.at < at.frames) {
                to.put(q.at + at.frame, q.value);
            }
            if let Some(v) = lane.value_at(at.frames - 1) {
                to.put(at.frame + at.frames - 1, v);
            }
        }
    }

    let mut ids: HashMap<String, String> = HashMap::new();
    for c in &piece.comments {
        ids.insert(c.id.clone(), id(&c.id));
    }
    for c in &piece.comments {
        let mut c = c.clone();
        c.id = ids[&c.id].clone();
        c.parent = c.parent.map(|q| ids.get(&q).cloned().unwrap_or(q));
        c.start += at.frame;
        c.end = c.end.map(|e| e + at.frame);
        p.comments.push(c);
    }
}

/// The song with its sections played in `order` (by marker id, repeats
/// allowed), written out one after another as a plain timeline. Any
/// bars before the first section stay at the start. The first time a
/// section comes it keeps its ids (and its comments and tab); repeats
/// are copies. `end` is where the song ends.
pub fn flatten(p: &Project, clock: Clock, order: &[String], end: i64) -> Project {
    let all = sections(p, end);
    let mut out = p.clone();
    let sounds: HashSet<String> = p.videos.iter().flat_map(|v| v.sound.clone()).collect();
    for t in &mut out.tracks {
        t.clips.retain(|c| sounds.contains(&c.id));
        t.note_clips.clear();
        for c in &mut t.comps {
            c.parts.clear();
        }
        for l in &mut t.automation {
            l.points.clear();
        }
    }
    out.markers.clear();
    out.tempo_changes.clear();
    out.meter_changes.clear();
    out.comments.clear();
    let (mut bar, mut tick, mut frame) = (1, 0, 0);
    let mut put = |out: &mut Project, from: i64, to: i64, fresh: bool| {
        let pc = piece(p, clock, from, to, end, !fresh);
        let len = bars_between(clock, from, to);
        let at = Bars {
            bar,
            tick,
            frame,
            ..len
        };
        paste_bars(out, &pc, &at, fresh);
        (bar, tick, frame) = (bar + len.count, tick + len.ticks, frame + len.frames);
    };
    if let Some(first) = all.first().filter(|s| s.from > 1) {
        put(&mut out, 1, first.from, false);
    }
    let mut seen = HashSet::new();
    for id in order {
        let Some(s) = all.iter().find(|s| &s.id == id) else {
            continue;
        };
        let fresh = !seen.insert(id.clone());
        put(&mut out, s.from, s.to, fresh);
    }
    // The picture stays where it was.
    out.videos = p.videos.clone();
    out.hit_points = p.hit_points.clone();
    out.timecode = p.timecode.clone();
    for (t, pt) in out.tracks.iter_mut().zip(&p.tracks) {
        t.clips.retain(|c| !sounds.contains(&c.id));
        t.clips
            .extend(pt.clips.iter().filter(|c| sounds.contains(&c.id)).cloned());
    }
    // Tab pinned to a section that is no longer played stays, unpinned.
    let kept: HashSet<&str> = out.markers.iter().map(|m| m.id.as_str()).collect();
    for tab in &mut out.tabs {
        if tab.marker.as_deref().is_some_and(|m| !kept.contains(m)) {
            tab.marker = None;
        }
    }
    out.arrangement.clear();
    // The loop and assignments stay on their bars, inside the new song.
    out.loop_bars = p.loop_bars;
    out.assignments = p.assignments.clone();
    let last = bar.max(2);
    out.loop_bars = (out.loop_bars.0.min(last - 1), out.loop_bars.1.min(last));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Clip, Marker, Track};

    /// 4/4 at 120 bpm, 48 kHz: 2 s a bar.
    const BAR: i64 = 96_000;
    const TBAR: i64 = crate::PPQ * 4;

    fn clock(bar: i64) -> (i64, i64) {
        ((bar - 1) * TBAR, (bar - 1) * BAR)
    }

    fn song() -> Project {
        let mut t = Track::new("t".into(), "Guitar".into());
        // A clip a section: verse bars 1–2, chorus 3–4, solo 5–6.
        t.clips = ["v", "c", "s"]
            .iter()
            .enumerate()
            .map(|(i, id)| Clip {
                id: id.to_string(),
                asset: format!("{id}.wav"),
                start: 2 * i as i64 * BAR,
                len: 2 * BAR,
                ..Clip::default()
            })
            .collect();
        let mk = |id: &str, bar| Marker {
            id: id.into(),
            bar,
            name: id.into(),
        };
        Project {
            tracks: vec![t],
            markers: vec![mk("verse", 1), mk("chorus", 3), mk("solo", 5)],
            ..Project::default()
        }
    }

    fn layout(p: &Project) -> Vec<(String, i64)> {
        let mut v: Vec<_> = (p.tracks[0].clips.iter())
            .map(|c| (c.asset.clone(), c.start / BAR))
            .collect();
        v.sort_by_key(|c| c.1);
        v
    }

    #[test]
    fn sections_run_from_marker_to_marker() {
        let s = sections(&song(), 7);
        let spans: Vec<(i64, i64)> = s.iter().map(|s| (s.from, s.to)).collect();
        assert_eq!(spans, [(1, 3), (3, 5), (5, 7)]);
    }

    #[test]
    fn a_play_order_is_written_out_with_repeats() {
        let p = song();
        let order: Vec<String> = ["verse", "chorus", "chorus", "solo", "verse"]
            .map(String::from)
            .to_vec();
        let out = flatten(&p, &clock, &order, 7);
        assert_eq!(
            layout(&out),
            [
                ("v.wav".into(), 0),
                ("c.wav".into(), 2),
                ("c.wav".into(), 4),
                ("s.wav".into(), 6),
                ("v.wav".into(), 8),
            ]
        );
        let m: Vec<(&str, i64)> = (out.markers.iter())
            .map(|m| (m.name.as_str(), m.bar))
            .collect();
        assert_eq!(
            m,
            [
                ("verse", 1),
                ("chorus", 3),
                ("chorus", 5),
                ("solo", 7),
                ("verse", 9)
            ]
        );
        // The first time keeps its ids; repeats are copies.
        assert_eq!(out.markers[1].id, "chorus");
        assert_ne!(out.markers[2].id, "chorus");
        assert_eq!(
            out.tracks[0].clips.iter().filter(|c| c.id == "c").count(),
            1
        );
    }

    #[test]
    fn a_section_copied_in_brings_its_tempo_and_puts_the_old_one_back_after() {
        let mut p = song();
        p.tempo_changes = vec![TempoChange {
            id: "t".into(),
            bar: 3,
            beat: 1,
            bpm: 90.0,
            glide: false,
        }];
        // A clock for 120 bpm then 90 from bar 3 (a bar at 90 is 8/3 s).
        let clock = |bar: i64| {
            let f = if bar <= 3 {
                (bar - 1) * BAR
            } else {
                2 * BAR + (bar - 3) * 128_000
            };
            ((bar - 1) * TBAR, f)
        };
        // Chorus (bars 3–4, at 90) pasted before the verse.
        let pc = piece(&p, &clock, 3, 5, 7, false);
        assert_eq!(pc.tempo_bpm, 90.0);
        let len = bars_between(&clock, 3, 5);
        let at = Bars {
            bar: 1,
            tick: 0,
            frame: 0,
            ..len
        };
        paste_bars(&mut p, &pc, &at, true);
        assert_eq!(p.tempo_bpm, 90.0);
        let t: Vec<(i64, f64)> = p.tempo_changes.iter().map(|c| (c.bar, c.bpm)).collect();
        // Back to 120 for the verse at bar 3, then 90 again at bar 5.
        assert_eq!(t, [(3, 120.0), (5, 90.0)]);
        assert_eq!(layout(&p)[0], ("c.wav".into(), 0));
        assert_eq!(
            p.tracks[0]
                .clips
                .iter()
                .find(|c| c.id == "v")
                .unwrap()
                .start,
            2 * 128_000
        );
    }
}
