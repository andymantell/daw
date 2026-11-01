// SPDX-License-Identifier: GPL-3.0-or-later
//! Linked copies on the timeline: which clips are instances of one clip
//! (see `daw_project::sync_links`), drawn as a group, and unlinking one.
//!
//! Each group has a colour of its own: a band along the top of every
//! instance, with a link mark and how many instances there are, and
//! the other instances of a selected clip are outlined in it.

use std::collections::{HashMap, HashSet};

use eframe::egui::{self, Color32, Rect, Stroke, StrokeKind, pos2, vec2};

use crate::timeline::Song;

/// A linked audio clip's group.
pub(crate) fn audio_key(link: &str) -> String {
    format!("a:{link}")
}

/// A note clip's group: its link, or the pattern it plays.
pub(crate) fn note_key(c: &daw_project::NoteClip) -> Option<String> {
    match (&c.pattern, &c.link) {
        (Some(p), _) => Some(format!("p:{p}")),
        (None, Some(l)) => Some(format!("n:{l}")),
        (None, None) => None,
    }
}

/// The groups on the timeline this frame.
#[derive(Default)]
pub(crate) struct Links {
    /// How many instances each group has.
    counts: HashMap<String, usize>,
    /// The groups a selected clip is in.
    selected: HashSet<String>,
}

impl Links {
    pub(crate) fn of(song: &Song, selected: &HashSet<u64>, note_sel: Option<&str>) -> Links {
        let mut l = Links::default();
        for lane in &song.lanes {
            for c in lane.clips.iter() {
                if let Some(link) = &c.link {
                    let k = audio_key(link);
                    if selected.contains(&c.id) {
                        l.selected.insert(k.clone());
                    }
                    *l.counts.entry(k).or_default() += 1;
                }
            }
            for c in lane.notes.iter() {
                if let Some(k) = note_key(c) {
                    if note_sel == Some(c.id.as_str()) {
                        l.selected.insert(k.clone());
                    }
                    *l.counts.entry(k).or_default() += 1;
                }
            }
        }
        l
    }

    /// How many instances `key`'s group has; 1 for a clip of its own.
    pub(crate) fn count(&self, key: Option<&str>) -> usize {
        key.and_then(|k| self.counts.get(k)).copied().unwrap_or(1)
    }

    /// Draws a linked clip's group: its band, link mark and count, and
    /// an outline when another instance is selected. `r` is the clip's
    /// rectangle as shown, `this_selected` whether it is selected
    /// itself. Nothing for a clip of its own.
    pub(crate) fn draw(
        &self,
        painter: &egui::Painter,
        key: Option<&str>,
        r: Rect,
        corner: f32,
        this_selected: bool,
    ) {
        let Some(key) = key else { return };
        let n = self.count(Some(key));
        if n < 2 {
            return;
        }
        let colour = colour(key);
        let p = painter.with_clip_rect(r.intersect(painter.clip_rect()));
        // The band along the top.
        let band = Rect::from_min_max(r.left_top(), pos2(r.right(), r.top() + 3.0));
        p.rect_filled(band, 0.0, colour);
        // The link mark and the count, at the top right, where there is
        // room.
        if r.width() >= 60.0 {
            let right = r.right() - 4.0;
            let mut x = right;
            if r.width() >= 90.0 {
                let font = egui::FontId::proportional(10.0);
                let g = p.layout_no_wrap(format!("×{n}"), font, colour);
                x -= g.size().x;
                p.galley(pos2(x, r.top() + 4.0), g, colour);
                x -= 3.0;
            }
            chain(&p, pos2(x - 6.0, r.top() + 10.0), colour);
        }
        // Too narrow to show an outline, it would only fill the clip in.
        if !this_selected && self.selected.contains(key) && r.width() >= 12.0 {
            painter.rect_stroke(r, corner, Stroke::new(2.0, colour), StrokeKind::Inside);
        }
    }
}

/// A chain link of two rings, centred on `at`, 12 points wide.
fn chain(p: &egui::Painter, at: egui::Pos2, colour: Color32) {
    let s = Stroke::new(1.5, colour);
    for dx in [-2.5, 2.5] {
        let r = Rect::from_center_size(at + vec2(dx, 0.0), vec2(7.0, 5.0));
        p.rect_stroke(r, 2.5, s, StrokeKind::Middle);
    }
}

/// A group's colour: picked from its key, bright enough to read on a
/// clip in light and dark themes.
pub(crate) fn colour(key: &str) -> Color32 {
    // FNV-1a, so the colour stays the same from run to run.
    let mut h: u32 = 0x811c_9dc5;
    for b in key.bytes() {
        h = (h ^ b as u32).wrapping_mul(0x0100_0193);
    }
    // Spread round the hue circle by the golden angle.
    let hue = ((h % 360) as f32 * 0.618_034).fract();
    egui::ecolor::Hsva::new(hue, 0.75, 0.95, 1.0).into()
}

/// Breaks the clip with group `key` (an audio clip `audio`, or note clip
/// `note`) off from its linked copies; one left alone stops being
/// linked too. True if it was linked.
pub(crate) fn unlink(song: &mut Song, audio: Option<u64>, note: Option<&str>) -> bool {
    let mut link = None;
    for lane in &mut song.lanes {
        if let Some(c) = lane.clips.iter_mut().find(|c| Some(c.id) == audio) {
            link = c.link.take().map(|l| audio_key(&l));
        }
        if let Some(c) = (lane.notes.iter_mut()).find(|c| Some(c.id.as_str()) == note)
            && c.pattern.is_none()
        {
            link = c.link.take().map(|l| format!("n:{l}"));
        }
    }
    let Some(key) = link else { return false };
    let mut left: Vec<&mut Option<String>> = Vec::new();
    for lane in &mut song.lanes {
        for c in lane.clips.iter_mut() {
            if c.link.as_deref().map(audio_key).as_deref() == Some(key.as_str()) {
                left.push(&mut c.link);
            }
        }
        for c in lane.notes.iter_mut() {
            if c.pattern.is_none()
                && c.link.as_deref().map(|l| format!("n:{l}")).as_deref() == Some(key.as_str())
            {
                left.push(&mut c.link);
            }
        }
    }
    if let [only] = left.as_mut_slice() {
        **only = None;
    }
    true
}

impl crate::App {
    /// The selected clips that have linked copies, by id in the project.
    pub(crate) fn selected_linked(&self) -> Vec<String> {
        let p = self.project();
        let sel = &self.timeline.selected;
        let note = self.timeline.note_sel.as_deref();
        let picked: Vec<String> = (self.tracks.iter())
            .flat_map(|t| {
                t.clips
                    .iter()
                    .filter(|c| sel.contains(&c.id))
                    .map(|c| c.key.clone())
            })
            .chain(note.map(String::from))
            .collect();
        let groups = daw_project::link_groups(&p);
        picked
            .into_iter()
            .filter(|id| {
                (groups.iter()).any(|g| !g.link.starts_with("pattern:") && g.clips.contains(id))
            })
            .collect()
    }

    /// Unlinks the selected clips from their linked copies (the
    /// `unlink-clips` command).
    pub(crate) fn unlink_selected(&mut self) {
        let ids = self.selected_linked();
        if ids.is_empty() {
            self.message = Some("Select a linked clip first.".into());
            return;
        }
        let _ = self.edit_model(|p| {
            for id in &ids {
                daw_project::unlink(p, id);
            }
            Ok(serde_json::Value::Null)
        });
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::Track;
    use crate::agent::driver::Driver;
    use crate::timing::tests::take;

    fn id(v: &Value, key: &str) -> String {
        v[key].as_str().unwrap().to_string()
    }

    fn ids(v: &Value) -> Vec<String> {
        (v.as_array().unwrap().iter())
            .map(|x| x.as_str().unwrap().to_string())
            .collect()
    }

    fn pitches(d: &mut Driver, c: &str) -> Vec<i64> {
        let v = d.call("get_note_clip", json!({"clip": c})).unwrap();
        (v["notes"].as_array().unwrap().iter())
            .map(|n| n["pitch"].as_i64().unwrap())
            .collect()
    }

    fn groups(d: &mut Driver) -> Vec<Vec<String>> {
        let p = d.project();
        (p["linked_groups"].as_array().unwrap().iter())
            .map(|g| ids(&g["clips"]))
            .collect()
    }

    #[test]
    fn an_assistant_copies_a_riff_as_linked_instances_and_unlinks_one() {
        let mut d = Driver::new();
        let t = id(
            &d.call("add_track", json!({"kind": "instrument"})).unwrap(),
            "track",
        );
        let a = id(
            &d.call(
                "add_note_clip",
                json!({"track": t, "bar": 1, "bars": 1,
                       "notes": [{"beat": 0, "beats": 1, "pitch": 36}]}),
            )
            .unwrap(),
            "clip",
        );
        // Copies are linked unless asked otherwise.
        let v = d
            .call("duplicate_clip", json!({"clip": a, "bars": [2, 3, 4]}))
            .unwrap();
        let copies = ids(&v["clips"]);
        assert!(v["link"].is_string());
        let lone = id(
            &d.call(
                "duplicate_clip",
                json!({"clip": a, "bar": 5, "linked": false}),
            )
            .unwrap(),
            "clip",
        );
        let mut group = vec![a.clone()];
        group.extend(copies.iter().cloned());
        assert_eq!(groups(&mut d), vec![group.clone()]);
        let info = d.call("get_note_clip", json!({"clip": copies[1]})).unwrap();
        assert_eq!(info["linked_with"].as_array().unwrap().len(), 3);

        // Editing the notes of one copy changes every instance, but not
        // the copy of its own.
        d.call(
            "edit_notes",
            json!({"clip": copies[1], "add": [{"beat": 2, "beats": 1, "pitch": 43}]}),
        )
        .unwrap();
        for c in &group {
            assert_eq!(pitches(&mut d, c), [36, 43], "{c}");
        }
        assert_eq!(pitches(&mut d, &lone), [36]);

        // Its place, length and mute are its own.
        d.call(
            "edit_clip",
            json!({"clip": copies[0], "muted": true, "len_beats": 2}),
        )
        .unwrap();
        let first = d.call("get_note_clip", json!({"clip": a})).unwrap();
        assert_eq!(first["muted"], false);
        assert_eq!(first["len_ticks"], 3840);

        // The whole change was one undo step.
        d.command("undo");
        d.command("undo");
        for c in &group {
            assert_eq!(pitches(&mut d, c), [36], "{c}");
        }
        d.command("redo");

        // Unlinked, a clip is edited on its own.
        let v = d.call("unlink_clip", json!({"clip": copies[2]})).unwrap();
        assert_eq!(ids(&v["unlinked"]), [copies[2].clone()]);
        d.call("edit_notes", json!({"clip": copies[2], "clear": true}))
            .unwrap();
        assert!(pitches(&mut d, &copies[2]).is_empty());
        assert_eq!(pitches(&mut d, &a), [36, 43]);
        assert_eq!(groups(&mut d)[0].len(), 3);

        // The command does the same for the selected clip; one left
        // alone stops being linked.
        d.app().timeline.note_sel = Some(copies[1].clone());
        d.command("unlink-clips");
        d.app().timeline.note_sel = Some(copies[0].clone());
        d.command("unlink-clips");
        assert!(groups(&mut d).is_empty());
        assert!(d.call("unlink_clip", json!({"clip": "nope"})).is_err());
    }

    #[test]
    fn audio_copies_share_gain_and_transpose_but_not_fades() {
        let mut d = Driver::new();
        let c = take(4_800, &[100]);
        let key = c.key.clone();
        let app = d.app();
        app.tracks.push(Track::new("Chops".into()));
        app.tracks[0].clips.push(c);
        app.rebuild();
        let v = d
            .call("duplicate_clip", json!({"clip": key, "bars": [3, 5]}))
            .unwrap();
        let copies = ids(&v["clips"]);
        d.call(
            "edit_clip",
            json!({"clip": copies[0], "gain_db": -4.0, "transpose_semitones": -6,
                   "fade_in_seconds": 0.01}),
        )
        .unwrap();
        let p = d.project();
        let clips = p["tracks"][0]["clips"].as_array().unwrap().clone();
        let of = |k: &str| clips.iter().find(|c| c["id"] == k).unwrap().clone();
        for k in [&key, &copies[1]] {
            assert_eq!(of(k)["gain_db"], -4.0);
            assert_eq!(of(k)["transpose_semitones"], -6.0);
        }
        let fades = |d: &mut Driver, k: &str| {
            (d.app().tracks[0].clips.iter())
                .find(|c| c.key == k)
                .map(|c| c.fade_in.len)
                .unwrap()
        };
        assert!(fades(&mut d, &copies[0]) > 0);
        assert_eq!(fades(&mut d, &key), 0);

        // Copy and paste makes linked copies too.
        let first = d.app().tracks[0].clips[0].id;
        d.app().timeline.selected = [first].into();
        d.command("copy");
        d.command("paste");
        assert_eq!(groups(&mut d)[0].len(), 4);
    }
}
