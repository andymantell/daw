// SPDX-License-Identifier: GPL-3.0-or-later
//! Parts of one project taken into another (roadmap Phase 4, plan A5):
//! chosen tracks with their clips, notes, effects, routing and audio,
//! the sections (markers), the tempo and time signature, and settings
//! such as the key. A teacher can reuse a lesson's backing track and
//! sections for another student's project.
//!
//! [`Project::part`] makes a new project holding just what was picked,
//! with new ids for everything in it, so bringing the same part in twice
//! (or from two projects made from the same template) never mixes the
//! copies up. It is also what "Export as a new project" saves, cut to a
//! range of bars with [`Project::cut_to`] if one was chosen.
//! [`Project::bring_in`] adds a part to a project.
//!
//! Anything a picked track points at that doesn't come along (a bus it
//! sends to, a sidechain from a track left behind) is let go of, so the
//! track plays to the master instead of to nothing.

use std::collections::{HashMap, HashSet};

use crate::{
    AutoPoint, Clip, ControlGroup, EditGroup, Effect, Lane, LaneTarget, Marker, MeterChange,
    MidiMap, PPQ, Pattern, Project, TabBlock, TempoChange, Track, new_id,
};

/// What to take from a project.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pick {
    /// The tracks, by id.
    pub tracks: Vec<String>,
    /// Also the buses and folders they play through, and theirs.
    pub routing: bool,
    /// The sections: its markers.
    pub markers: bool,
    /// The tempo and time signature, with their changes.
    pub tempo: bool,
    /// The key and scale, the tuning reference and the loop.
    pub settings: bool,
    /// Lesson notes and tab blocks.
    pub notes: bool,
}

impl Pick {
    /// Everything: every track and all the rest.
    pub fn all(p: &Project) -> Pick {
        Pick {
            tracks: p.tracks.iter().map(|t| t.id.clone()).collect(),
            routing: true,
            markers: true,
            tempo: true,
            settings: true,
            notes: true,
        }
    }
}

/// What [`Project::bring_in`] added.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Brought {
    /// The new tracks' ids, in order.
    pub tracks: Vec<String>,
    /// How many of the markers were new here.
    pub markers: usize,
}

/// A range of bars to cut a part to, with where it starts and ends in
/// frames (at the part's rate) and in ticks, as the tempo map has them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    /// From the start of the first bar to the start of the second,
    /// counted from 1.
    pub bars: (i64, i64),
    pub frames: (i64, i64),
    pub ticks: (i64, i64),
}

impl Track {
    /// Every sound file the track uses: its clips', its frozen audio,
    /// and its effects' and instruments'. Asset names, or full paths
    /// for files not yet saved into a project.
    pub fn assets(&self) -> Vec<String> {
        let mut v: Vec<String> = (self.clips.iter().map(|c| c.asset.clone()))
            .chain(self.frozen.iter().map(|f| f.asset.clone()))
            .chain(
                (self.effects.iter())
                    .chain(&self.instrument)
                    .chain(self.layers.iter().map(|l| &l.instrument))
                    .flat_map(Effect::assets)
                    .map(String::from),
            )
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// Renames every sound file the track uses with `f`: to point them
    /// at where the files are now.
    pub fn rename_assets(&mut self, f: &impl Fn(&str) -> String) {
        for c in &mut self.clips {
            c.asset = f(&c.asset);
        }
        if let Some(fr) = &mut self.frozen {
            fr.asset = f(&fr.asset);
            for c in &mut fr.clips {
                c.asset = f(&c.asset);
            }
        }
        let instruments =
            (self.instrument.iter_mut()).chain(self.layers.iter_mut().map(|l| &mut l.instrument));
        for e in self.effects.iter_mut().chain(instruments) {
            rename_effect_assets(e, f);
        }
    }
}

fn rename_effect_assets(e: &mut Effect, f: &impl Fn(&str) -> String) {
    if e.asset().is_some() {
        e.response = f(&e.response);
    }
    for (_, s) in &mut e.samples {
        if !s.is_empty() && !s.starts_with("made:") {
            *s = f(s);
        }
    }
    for inner in &mut e.inner {
        rename_effect_assets(inner, f);
    }
}

impl Project {
    /// The tracks `pick` names, with (if it asks) the buses and folders
    /// they play through, in this project's order.
    pub fn picked_tracks(&self, pick: &Pick) -> Vec<&Track> {
        let mut want: HashSet<&str> = pick.tracks.iter().map(String::as_str).collect();
        if pick.routing {
            let mut todo: Vec<&str> = want.iter().copied().collect();
            while let Some(id) = todo.pop() {
                let Some(t) = self.tracks.iter().find(|t| t.id == id) else {
                    continue;
                };
                let to = (t.parent.iter().chain(&t.output))
                    .chain(t.sends.iter().map(|s| &s.to))
                    .chain(t.instrument_outputs.iter().map(|o| &o.to));
                for to in to {
                    if self.tracks.iter().any(|t| t.id == *to) && want.insert(to) {
                        todo.push(to);
                    }
                }
            }
        }
        (self.tracks.iter())
            .filter(|t| want.contains(t.id.as_str()))
            .collect()
    }

    /// A new project at this one's rate holding just what `pick` names,
    /// with new ids throughout. Without `pick.tempo` it keeps the default
    /// tempo; [`bring_in`](Self::bring_in) then leaves the other
    /// project's alone.
    pub fn part(&self, pick: &Pick) -> Project {
        let mut p = Project {
            rate: self.rate,
            ..Project::default()
        };
        if pick.tempo {
            p.tempo_bpm = self.tempo_bpm;
            p.beats_per_bar = self.beats_per_bar;
            p.beat_unit = self.beat_unit;
            p.tempo_changes = (self.tempo_changes.iter())
                .map(|c| TempoChange {
                    id: new_id(),
                    ..c.clone()
                })
                .collect();
            p.meter_changes = (self.meter_changes.iter())
                .map(|c| MeterChange {
                    id: new_id(),
                    ..c.clone()
                })
                .collect();
        }
        if pick.settings {
            p.scale = self.scale;
            p.reference_hz = self.reference_hz;
            p.loop_bars = self.loop_bars;
        }
        let mut markers = HashMap::new();
        if pick.markers {
            for m in &self.markers {
                let id = new_id();
                markers.insert(m.id.clone(), id.clone());
                p.markers.push(Marker { id, ..m.clone() });
            }
        }
        if pick.notes {
            p.notes = self.notes.clone();
            p.tabs = (self.tabs.iter())
                .map(|b| TabBlock {
                    id: new_id(),
                    marker: b.marker.as_ref().and_then(|m| markers.get(m).cloned()),
                    ..b.clone()
                })
                .collect();
        }

        let picked = self.picked_tracks(pick);
        let ids: HashMap<String, String> =
            (picked.iter()).map(|t| (t.id.clone(), new_id())).collect();
        // Patterns and control groups the tracks use come too.
        let mut patterns: HashMap<String, String> = HashMap::new();
        let mut groups: HashMap<String, String> = HashMap::new();
        for t in &picked {
            for c in &t.note_clips {
                if let Some(pat) = &c.pattern
                    && self.patterns.iter().any(|x| x.id == *pat)
                {
                    patterns.entry(pat.clone()).or_insert_with(new_id);
                }
            }
            if let Some(g) = &t.group
                && self.control_groups.iter().any(|x| x.id == *g)
            {
                groups.entry(g.clone()).or_insert_with(new_id);
            }
        }
        p.patterns = (self.patterns.iter())
            .filter_map(|x| {
                Some(Pattern {
                    id: patterns.get(&x.id)?.clone(),
                    ..x.clone()
                })
            })
            .collect();
        p.control_groups = (self.control_groups.iter())
            .filter_map(|g| {
                Some(ControlGroup {
                    id: groups.get(&g.id)?.clone(),
                    ..g.clone()
                })
            })
            .collect();
        p.tracks = (picked.iter())
            .map(|t| renewed(t, &ids, &patterns, &groups, self.rate))
            .collect();
        // Groups and controller ties among the tracks that came.
        p.edit_groups = (self.edit_groups.iter())
            .filter_map(|g| {
                let mut tracks: Vec<String> = g
                    .tracks
                    .iter()
                    .filter_map(|t| ids.get(t).cloned())
                    .collect();
                tracks.sort();
                (tracks.len() >= 2).then(|| EditGroup {
                    id: new_id(),
                    tracks,
                    ..g.clone()
                })
            })
            .collect();
        p.midi_maps = (self.midi_maps.iter())
            .filter_map(|m| {
                let track = ids.get(&m.track)?.clone();
                let param = renamed_param(&m.param, &ids, &effect_ids(self, &m.track, &p, &track))?;
                Some(MidiMap {
                    id: new_id(),
                    track,
                    param,
                    ..m.clone()
                })
            })
            .collect();
        p
    }

    /// Cuts the project to `range`: what is in it moves to the start,
    /// clips and notes across its ends are cut there, and the tempo and
    /// time signature at its start become the song's. Frozen audio is
    /// let go of, as the clips it was made from have changed.
    pub fn cut_to(&mut self, range: Range) {
        let (a, b) = range.bars;
        let (fa, fb) = range.frames;
        let (ta, tb) = range.ticks;
        let shift = a - 1;
        // The tempo and time signature in force at the start of bar `a`.
        if let Some(c) =
            (self.tempo_changes.iter()).rfind(|c| c.bar < a || (c.bar == a && c.beat == 0))
        {
            self.tempo_bpm = c.bpm;
        }
        if let Some(m) = self.meter_changes.iter().rfind(|m| m.bar <= a) {
            self.beats_per_bar = m.beats;
            self.beat_unit = m.unit;
        }
        self.tempo_changes
            .retain(|c| (c.bar > a || (c.bar == a && c.beat > 0)) && c.bar < b);
        for c in &mut self.tempo_changes {
            c.bar -= shift;
        }
        self.meter_changes.retain(|m| m.bar > a && m.bar < b);
        for m in &mut self.meter_changes {
            m.bar -= shift;
        }
        let kept: HashSet<String> = (self.markers.iter())
            .filter(|m| m.bar >= a && m.bar < b)
            .map(|m| m.id.clone())
            .collect();
        self.markers.retain(|m| kept.contains(&m.id));
        for m in &mut self.markers {
            m.bar -= shift;
        }
        for t in &mut self.tabs {
            t.marker = t.marker.take().filter(|m| kept.contains(m));
        }
        let (la, lb) = self.loop_bars;
        self.loop_bars = if la >= a && lb <= b && la < lb {
            (la - shift, lb - shift)
        } else {
            (1, b - shift)
        };
        for l in &mut self.master.automation {
            cut_lane(l, fa, fb);
        }
        for t in &mut self.tracks {
            t.frozen = None;
            t.clips = (t.clips.iter())
                .filter_map(|c| cut_clip(c, fa, fb))
                .collect();
            for c in &mut t.comps {
                c.parts.retain(|p| p.end > fa && p.start < fb);
                for p in &mut c.parts {
                    p.start = p.start.max(fa) - fa;
                    p.end = p.end.min(fb) - fa;
                }
            }
            for l in &mut t.automation {
                cut_lane(l, fa, fb);
            }
            t.note_clips.retain(|c| c.end() > ta && c.start < tb);
            for c in &mut t.note_clips {
                let start = c.start.max(ta);
                let end = c.end().min(tb);
                c.offset += start - c.start;
                c.start = start - ta;
                c.len = end - start;
            }
        }
    }

    /// The project at `rate`: positions in frames moved to match, so
    /// everything plays at the same time as before. Frozen audio keeps
    /// its own rate.
    pub fn at_rate(mut self, rate: u32) -> Project {
        if rate == self.rate || self.rate == 0 {
            self.rate = rate;
            return self;
        }
        let k = rate as f64 / self.rate as f64;
        let s = |v: i64| (v as f64 * k).round() as i64;
        for l in &mut self.master.automation {
            for p in &mut l.points {
                p.at = s(p.at);
            }
        }
        for t in &mut self.tracks {
            for c in &mut t.clips {
                c.start = s(c.start);
                c.offset = s(c.offset);
                c.len = s(c.len);
                c.fade_in.len = s(c.fade_in.len);
                c.fade_out.len = s(c.fade_out.len);
                for p in &mut c.envelope {
                    p.at = s(p.at);
                }
            }
            for c in &mut t.comps {
                for p in &mut c.parts {
                    p.start = s(p.start);
                    p.end = s(p.end);
                }
            }
            for l in &mut t.automation {
                for p in &mut l.points {
                    p.at = s(p.at);
                }
            }
        }
        for c in &mut self.comments {
            c.start = s(c.start);
            c.end = c.end.map(s);
        }
        for h in &mut self.hit_points {
            h.at = s(h.at);
        }
        for v in &mut self.videos {
            v.start = s(v.start);
            v.offset = s(v.offset);
            v.len = s(v.len);
        }
        self.timecode.offset = s(self.timecode.offset);
        self.rate = rate;
        self
    }

    /// Adds `part` (made by [`part`](Self::part) from another project)
    /// to this one: its tracks after these, its markers among these
    /// (those already here at the same bar with the same name are kept
    /// once), its lesson notes after these, and, if `pick` asks, its
    /// tempo and settings in place of these.
    pub fn bring_in(&mut self, part: Project, pick: &Pick) -> Brought {
        let part = part.at_rate(self.rate);
        if pick.tempo {
            self.tempo_bpm = part.tempo_bpm;
            self.beats_per_bar = part.beats_per_bar;
            self.beat_unit = part.beat_unit;
            self.tempo_changes = part.tempo_changes;
            self.meter_changes = part.meter_changes;
        }
        if pick.settings {
            self.scale = part.scale;
            self.reference_hz = part.reference_hz;
            self.loop_bars = part.loop_bars;
        }
        // Markers already here stand in for the same ones coming in.
        let mut same = HashMap::new();
        let mut markers = 0;
        for m in part.markers {
            match (self.markers.iter()).find(|x| x.bar == m.bar && x.name == m.name) {
                Some(x) => {
                    same.insert(m.id, x.id.clone());
                }
                None => {
                    self.markers.push(m);
                    markers += 1;
                }
            }
        }
        self.markers.sort_by_key(|m| m.bar);
        if !part.notes.trim().is_empty() {
            if self.notes.trim().is_empty() {
                self.notes = part.notes;
            } else {
                self.notes = format!("{}\n\n{}", self.notes.trim_end(), part.notes);
            }
        }
        for mut b in part.tabs {
            if let Some(m) = &b.marker
                && let Some(x) = same.get(m)
            {
                b.marker = Some(x.clone());
            }
            self.tabs.push(b);
        }
        let tracks = part.tracks.iter().map(|t| t.id.clone()).collect();
        self.tracks.extend(part.tracks);
        self.patterns.extend(part.patterns);
        self.control_groups.extend(part.control_groups);
        self.edit_groups.extend(part.edit_groups);
        self.midi_maps.extend(part.midi_maps);
        Brought { tracks, markers }
    }
}

/// The effect ids of track `old` in `from` paired with their new ones in
/// track `new` of `to`, which was made from it, in order.
fn effect_ids(from: &Project, old: &str, to: &Project, new: &str) -> HashMap<String, String> {
    let (Some(a), Some(b)) = (
        from.tracks.iter().find(|t| t.id == old),
        to.tracks.iter().find(|t| t.id == new),
    ) else {
        return HashMap::new();
    };
    let list = |t: &Track| -> Vec<String> {
        (t.effects.iter().map(|e| e.id.clone()))
            .chain(t.instrument.iter().map(|e| e.id.clone()))
            .chain(t.layers.iter().map(|l| l.id.clone()))
            .chain(t.layers.iter().map(|l| l.instrument.id.clone()))
            .collect()
    };
    list(a).into_iter().zip(list(b)).collect()
}

/// A lane's (or a controller tie's) setting with its bus or effect
/// renamed, or `None` if what it names didn't come along.
fn renamed_param(
    param: &str,
    tracks: &HashMap<String, String>,
    effects: &HashMap<String, String>,
) -> Option<String> {
    let lane = Lane::new(param);
    Some(match lane.target() {
        LaneTarget::Send(bus) => format!("{}{}", Lane::SEND, tracks.get(bus)?),
        LaneTarget::Fx { effect, key } => format!("{}{}:{key}", Lane::FX, effects.get(effect)?),
        LaneTarget::Plugin { effect, id } => {
            format!("{}{}:{id}", Lane::PLUGIN, effects.get(effect)?)
        }
        _ => param.to_string(),
    })
}

/// A copy of `t` with new ids, pointing only at what is in `tracks`
/// (old id to new).
fn renewed(
    t: &Track,
    tracks: &HashMap<String, String>,
    patterns: &HashMap<String, String>,
    groups: &HashMap<String, String>,
    rate: u32,
) -> Track {
    let mut n = t.clone();
    n.id = tracks[&t.id].clone();
    let track = |id: &Option<String>| id.as_ref().and_then(|i| tracks.get(i).cloned());
    n.parent = track(&t.parent);
    n.output = track(&t.output);
    n.group = t.group.as_ref().and_then(|g| groups.get(g).cloned());
    n.sends = (t.sends.iter())
        .filter_map(|s| {
            let mut s = s.clone();
            s.to = tracks.get(&s.to)?.clone();
            Some(s)
        })
        .collect();
    n.instrument_outputs = (t.instrument_outputs.iter())
        .filter_map(|o| {
            let mut o = o.clone();
            o.to = tracks.get(&o.to)?.clone();
            Some(o)
        })
        .collect();
    // Effects, instruments and layers: containers and lanes follow.
    let mut fx: HashMap<String, String> = HashMap::new();
    let mut fresh = |id: &mut String| {
        let new = new_id();
        fx.insert(std::mem::replace(id, new.clone()), new);
    };
    for e in &mut n.effects {
        fresh(&mut e.id);
    }
    if let Some(i) = &mut n.instrument {
        fresh(&mut i.id);
    }
    for l in &mut n.layers {
        fresh(&mut l.id);
        fresh(&mut l.instrument.id);
    }
    for e in &mut n.effects {
        e.container = e.container.as_ref().and_then(|c| fx.get(c).cloned());
        e.sidechain = track(&e.sidechain).filter(|s| *s != n.id);
    }
    n.automation = (t.automation.iter())
        .filter_map(|l| {
            Some(Lane {
                param: renamed_param(&l.param, tracks, &fx)?,
                points: l.points.clone(),
            })
        })
        .collect();
    let mut comps = HashMap::new();
    for c in &mut n.comps {
        let id = new_id();
        comps.insert(std::mem::replace(&mut c.id, id.clone()), id);
    }
    n.comp = t.comp.as_ref().and_then(|c| comps.get(c).cloned());
    for c in &mut n.clips {
        c.id = new_id();
    }
    for c in &mut n.note_clips {
        c.id = new_id();
        c.pattern = c.pattern.as_ref().and_then(|p| patterns.get(p).cloned());
    }
    // Frozen audio still fits, under the effects' new names.
    let print = n.freeze_print(rate);
    if let Some(f) = &mut n.frozen {
        if f.print == t.freeze_print(rate) {
            f.print = print;
        }
        for (id, _) in &mut f.chain {
            if let Some(new) = fx.get(id) {
                *id = new.clone();
            }
        }
    }
    n
}

/// The part of `c` between frames `fa` and `fb`, moved to start at
/// `fa`, or `None` if none of it is there.
fn cut_clip(c: &Clip, fa: i64, fb: i64) -> Option<Clip> {
    let end = c.start + c.len;
    if end <= fa || c.start >= fb {
        return None;
    }
    let start = c.start.max(fa);
    let end = end.min(fb);
    let mut n = c.clone();
    n.offset += start - c.start;
    n.start = start - fa;
    n.len = end - start;
    if start > c.start {
        n.fade_in.len = 0;
    }
    if end < c.start + c.len {
        n.fade_out.len = 0;
    }
    n.fade_in.len = n.fade_in.len.min(n.len);
    n.fade_out.len = n.fade_out.len.min(n.len - n.fade_in.len);
    Some(n)
}

/// Cuts a lane to frames `fa` to `fb`, moved to start at `fa`, keeping
/// its values at the ends.
fn cut_lane(l: &mut Lane, fa: i64, fb: i64) {
    let (Some(first), Some(last)) = (l.value_at(fa), l.value_at(fb)) else {
        return;
    };
    let mut points = vec![AutoPoint {
        at: 0,
        value: first,
    }];
    points.extend(
        (l.points.iter())
            .filter(|p| p.at > fa && p.at < fb)
            .map(|p| AutoPoint {
                at: p.at - fa,
                value: p.value,
            }),
    );
    points.push(AutoPoint {
        at: fb - fa,
        value: last,
    });
    l.points = points;
}

/// The ticks at the start of bar `bar` (from 1) with a steady time
/// signature: for tests and for projects without meter changes.
pub fn bar_ticks(bar: i64, beats_per_bar: u32, beat_unit: u32) -> i64 {
    (bar - 1) * beats_per_bar as i64 * PPQ * 4 / beat_unit.max(1) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Comp, CompPart, EffectState, Frozen, InstrumentOutput, Note, NoteClip, Send, TrackKind,
    };

    fn clip(id: &str, asset: &str, start: i64, len: i64) -> Clip {
        Clip {
            id: id.into(),
            asset: asset.into(),
            name: id.into(),
            start,
            len,
            ..Clip::default()
        }
    }

    /// A lesson: a guitar track through a reverb bus, a drum track with
    /// a compressor keyed from the guitar, sections, tempo and notes.
    fn lesson() -> Project {
        let mut bus = Track::new("bus".into(), "Reverb bus".into());
        bus.kind = TrackKind::Bus;
        let mut gtr = Track::new("gtr".into(), "Guitar".into());
        gtr.clips.push(clip("c1", "aaaa.wav", 0, 48_000));
        gtr.sends.push(Send::to("bus".into()));
        gtr.effects.push(Effect {
            id: "drive".into(),
            name: "Drive".into(),
            class: vec![7; 16],
            state: Some(EffectState {
                component: vec![1, 2, 3],
                controller: None,
            }),
            ..Effect::default()
        });
        gtr.automation.push(Lane {
            param: "plugin:drive:4".into(),
            points: vec![AutoPoint { at: 0, value: 0.5 }],
        });
        gtr.automation.push(Lane {
            param: "send:bus".into(),
            points: vec![AutoPoint { at: 0, value: -6.0 }],
        });
        let mut drums = Track::new("drums".into(), "Drums".into());
        drums.effects.push(Effect {
            id: "comp".into(),
            builtin: "compressor".into(),
            sidechain: Some("gtr".into()),
            ..Effect::default()
        });
        drums.note_clips.push(NoteClip {
            notes: vec![Note::new(0, 240, 36, 100)],
            ..NoteClip::new("n1".into(), "Beat".into(), 0, 4 * PPQ)
        });
        Project {
            tempo_bpm: 90.0,
            beats_per_bar: 3,
            tempo_changes: vec![TempoChange {
                id: "t1".into(),
                bar: 5,
                beat: 0,
                bpm: 100.0,
                glide: false,
            }],
            markers: vec![
                Marker {
                    id: "m1".into(),
                    bar: 1,
                    name: "Warm-up".into(),
                },
                Marker {
                    id: "m2".into(),
                    bar: 5,
                    name: "Riff".into(),
                },
            ],
            notes: "# Riff\n- slow".into(),
            tabs: vec![TabBlock {
                id: "b1".into(),
                marker: Some("m2".into()),
                title: "Riff".into(),
                text: "e|--3--|".into(),
                author: String::new(),
                created: 1,
            }],
            tracks: vec![bus, gtr, drums],
            ..Project::default()
        }
    }

    fn pick(tracks: &[&str]) -> Pick {
        Pick {
            tracks: tracks.iter().map(|t| t.to_string()).collect(),
            ..Pick::default()
        }
    }

    #[test]
    fn a_track_comes_with_new_ids_and_its_audio_named() {
        let src = lesson();
        let part = src.part(&pick(&["gtr"]));
        assert_eq!(part.tracks.len(), 1);
        let t = &part.tracks[0];
        assert_eq!(t.name, "Guitar");
        assert_ne!(t.id, "gtr");
        assert_ne!(t.clips[0].id, "c1");
        assert_ne!(t.effects[0].id, "drive");
        assert_eq!(t.effects[0].state, src.tracks[1].effects[0].state);
        assert_eq!(t.assets(), ["aaaa.wav"]);
        // The lane follows its effect; the bus didn't come, so neither
        // does the send or its lane.
        assert_eq!(t.automation.len(), 1);
        assert_eq!(t.automation[0].effect(), Some(t.effects[0].id.as_str()));
        assert!(t.sends.is_empty());
        // Nothing else was asked for.
        assert!(part.markers.is_empty() && part.notes.is_empty());
        assert_eq!(part.tempo_bpm, Project::default().tempo_bpm);
    }

    #[test]
    fn the_buses_a_track_plays_through_can_come_too() {
        let src = lesson();
        let part = src.part(&Pick {
            routing: true,
            ..pick(&["gtr"])
        });
        let names: Vec<_> = part.tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Reverb bus", "Guitar"]);
        let (bus, gtr) = (&part.tracks[0], &part.tracks[1]);
        assert_eq!(gtr.sends[0].to, bus.id);
        assert_eq!(gtr.automation[1].param, format!("send:{}", bus.id));
    }

    #[test]
    fn a_sidechain_from_a_track_left_behind_is_let_go() {
        let part = lesson().part(&pick(&["drums"]));
        assert_eq!(part.tracks[0].effects[0].sidechain, None);
        let both = lesson().part(&pick(&["gtr", "drums"]));
        assert_eq!(
            both.tracks[1].effects[0].sidechain.as_ref(),
            Some(&both.tracks[0].id)
        );
    }

    #[test]
    fn bringing_in_adds_tracks_markers_tempo_and_notes() {
        let src = lesson();
        let mut dest = Project {
            markers: vec![Marker {
                id: "x".into(),
                bar: 5,
                name: "Riff".into(),
            }],
            notes: "# This week".into(),
            tracks: vec![Track::new("s".into(), "Student".into())],
            ..Project::default()
        };
        let p = Pick::all(&src);
        let got = dest.bring_in(src.part(&p), &p);
        assert_eq!(got.tracks.len(), 3);
        assert_eq!(dest.tracks.len(), 4);
        assert_eq!(dest.tracks[0].name, "Student");
        // The same section already here is kept once, and the tab block
        // pinned to it follows.
        assert_eq!(got.markers, 1);
        let bars: Vec<_> = dest
            .markers
            .iter()
            .map(|m| (m.bar, m.name.as_str()))
            .collect();
        assert_eq!(bars, [(1, "Warm-up"), (5, "Riff")]);
        assert_eq!(dest.tabs[0].marker.as_deref(), Some("x"));
        assert_eq!(dest.tempo_bpm, 90.0);
        assert_eq!(dest.beats_per_bar, 3);
        assert_eq!(dest.tempo_changes[0].bpm, 100.0);
        assert_eq!(dest.notes, "# This week\n\n# Riff\n- slow");
    }

    #[test]
    fn bringing_the_same_part_in_twice_makes_two_copies() {
        let src = lesson();
        let mut dest = Project::default();
        let p = pick(&["gtr"]);
        dest.bring_in(src.part(&p), &p);
        dest.bring_in(src.part(&p), &p);
        assert_eq!(dest.tracks.len(), 2);
        assert_ne!(dest.tracks[0].id, dest.tracks[1].id);
        assert_ne!(dest.tracks[0].effects[0].id, dest.tracks[1].effects[0].id);
        // Without the tempo picked, the project's own stays.
        assert_eq!(dest.tempo_bpm, 120.0);
    }

    #[test]
    fn audio_from_a_project_at_another_rate_plays_at_the_same_time() {
        let mut src = lesson();
        src.rate = 44_100;
        src.tracks[1].clips[0] = clip("c1", "aaaa.wav", 44_100, 22_050);
        let mut dest = Project::default();
        let p = pick(&["gtr"]);
        dest.bring_in(src.part(&p), &p);
        let c = &dest.tracks[0].clips[0];
        assert_eq!((c.start, c.len), (48_000, 24_000));
        // Notes are in ticks, which don't change.
        let mut dest = Project::default();
        let p = pick(&["drums"]);
        dest.bring_in(src.part(&p), &p);
        assert_eq!(dest.tracks[0].note_clips[0].len, 4 * PPQ);
    }

    #[test]
    fn layers_containers_comps_and_outputs_follow_their_new_ids() {
        let mut t = Track::new("t".into(), "Keys".into());
        t.kind = TrackKind::Instrument;
        t.instrument = Some(Effect {
            id: "inst".into(),
            builtin: "drums".into(),
            samples: vec![
                ("pad1".into(), "bbbb.wav".into()),
                ("pad2".into(), "made:kick".into()),
            ],
            ..Effect::default()
        });
        t.layers.push(crate::Layer::new(
            "layer".into(),
            Effect {
                id: "li".into(),
                builtin: "synth".into(),
                ..Effect::default()
            },
        ));
        t.effects.push(Effect {
            id: "e".into(),
            builtin: "delay".into(),
            container: Some("layer".into()),
            ..Effect::default()
        });
        t.instrument_outputs.push(InstrumentOutput {
            output: 2,
            to: "pads".into(),
        });
        t.comps.push(Comp {
            id: "k".into(),
            name: "Best".into(),
            parts: vec![CompPart {
                take: 0,
                start: 0,
                end: 10,
            }],
        });
        t.comp = Some("k".into());
        let mut pads = Track::new("pads".into(), "Pads".into());
        pads.kind = TrackKind::Bus;
        let src = Project {
            tracks: vec![t, pads],
            ..Project::default()
        };
        let part = src.part(&Pick {
            routing: true,
            ..pick(&["t"])
        });
        let t = &part.tracks[0];
        assert_eq!(t.effects[0].container.as_ref(), Some(&t.layers[0].id));
        assert_eq!(t.comp.as_ref(), Some(&t.comps[0].id));
        assert_ne!(t.comps[0].id, "k");
        assert_eq!(t.instrument_outputs[0].to, part.tracks[1].id);
        assert_eq!(t.assets(), ["bbbb.wav"]);
    }

    #[test]
    fn frozen_audio_still_fits_under_new_ids() {
        let mut src = lesson();
        let t = &mut src.tracks[1];
        let mut f = t.freeze_shape(src.rate);
        f.asset = "ffff.wav".into();
        f.print = t.freeze_print(src.rate);
        t.frozen = Some(f);
        let part = src.part(&pick(&["gtr"]));
        let t = &part.tracks[0];
        let f = t.frozen.as_ref().unwrap();
        assert!(f.fits(t, part.rate));
        assert_eq!(f.print, t.freeze_print(part.rate));
        assert!(t.assets().contains(&"ffff.wav".to_string()));
    }

    #[test]
    fn assets_can_be_pointed_elsewhere() {
        let mut t = lesson().tracks[1].clone();
        t.frozen = Some(Frozen {
            asset: "ffff.wav".into(),
            rate: 48_000,
            start: 0,
            len: 1,
            print: String::new(),
            clips: Vec::new(),
            chain: Vec::new(),
        });
        t.effects.push(Effect {
            builtin: "reverb".into(),
            response: "room:hall".into(),
            ..Effect::default()
        });
        t.rename_assets(&|a| format!("/somewhere/{a}"));
        assert_eq!(t.assets(), ["/somewhere/aaaa.wav", "/somewhere/ffff.wav"]);
        assert_eq!(t.effects[1].response, "room:hall");
    }

    #[test]
    fn cutting_to_bars_moves_them_to_the_start() {
        let mut p = lesson();
        // 3/4 at 90 bpm: a beat is 32_000 frames, a bar 96_000.
        let bar = 96_000;
        p.tracks[1].clips = vec![
            clip("a", "aaaa.wav", 0, 3 * bar),
            clip("b", "aaaa.wav", 5 * bar, bar),
        ];
        p.tracks[1].automation[1].points = vec![
            AutoPoint { at: 0, value: 0.0 },
            AutoPoint {
                at: 4 * bar,
                value: -10.0,
            },
        ];
        p.loop_bars = (2, 3);
        let ticks = |b| bar_ticks(b, 3, 4);
        p.cut_to(Range {
            bars: (2, 4),
            frames: (bar, 3 * bar),
            ticks: (ticks(2), ticks(4)),
        });
        let c = &p.tracks[1].clips;
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].start, c[0].offset, c[0].len), (0, bar, 2 * bar));
        let lane = &p.tracks[1].automation[1];
        assert_eq!(lane.points.first().unwrap().value, -2.5);
        assert_eq!(lane.points.last().unwrap().at, 2 * bar);
        assert_eq!(lane.points.last().unwrap().value, -7.5);
        // The drum clip (one bar and a beat of 3/4) is cut to what is
        // past bar 2.
        let n = &p.tracks[2].note_clips[0];
        assert_eq!((n.start, n.offset, n.len), (0, 3 * PPQ, PPQ));
        assert!(p.markers.is_empty(), "no section starts in bars 2 to 3");
        assert_eq!(p.tabs[0].marker, None);
        assert_eq!(p.loop_bars, (1, 2));
        assert!(p.tempo_changes.is_empty());
    }

    #[test]
    fn cutting_from_a_tempo_change_starts_at_its_tempo() {
        let mut p = lesson();
        p.cut_to(Range {
            bars: (5, 9),
            frames: (0, 1),
            ticks: (0, 1),
        });
        assert_eq!(p.tempo_bpm, 100.0);
        assert!(p.tempo_changes.is_empty());
        assert_eq!(p.markers.len(), 1);
        assert_eq!((p.markers[0].bar, p.markers[0].name.as_str()), (1, "Riff"));
        assert_eq!(p.tabs[0].marker.as_ref(), Some(&p.markers[0].id));
        assert_eq!(p.loop_bars, (1, 5));
    }
}
