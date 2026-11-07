// SPDX-License-Identifier: GPL-3.0-or-later
//! Gluing clips into one: several clips side by side on one track
//! become a single clip in their place, which moves, copies, loops and
//! fades as one piece.
//!
//! - **Note clips** are merged straight away: the new clip holds the
//!   notes and controller lanes they played, where they played them
//!   (see `daw_project::glue_note_clips`).
//! - **Audio clips** are rendered, the way a bounce is (`render.rs`),
//!   but from their own audio only: their volume, fades, stretching and
//!   pitch edits are kept, and the track's effects and fader go on
//!   playing the new clip as they played the old ones. Gaps between
//!   them become silence. The clip comes in when the render is done.
//!
//! The `glue` command glues the selected audio clips, track by track;
//! with one clip selected (audio or notes) it glues it to the next clip
//! on its track. Assistants use `glue_clips`. Each glue is one undo
//! step.

use serde_json::{Value, json};

use crate::App;
use crate::agent::{Access, Tool};
use crate::timeline::Clip;

type Done = Result<(Value, Option<String>), String>;

pub(crate) const TOOLS: &[Tool] = &[Tool {
    name: "glue_clips",
    description: "Glues clips side by side on one track into one clip in their place, \
                  which then moves, copies, loops and fades as one piece. Give two clips \
                  or more, all audio or all notes. Note clips: the new clip holds the \
                  notes and controller lanes they played, where they played them (each \
                  time round of a looping clip), and keeps the first one's name. Audio \
                  clips: their own audio, with each clip's volume, fades, stretching and \
                  transpose, is rendered into one new clip in the background (gaps become \
                  silence); the track's effects and fader still play it. Muted clips, \
                  drum pattern clips and clips with another clip lying wholly between them \
                  can't be glued. Gives the new clip's id, also for audio, which shows in \
                  get_project once rendering is 0.",
    access: Access::Edit,
    schema: || {
        json!({"type": "object", "properties": {
            "clips": {"type": "array", "items": {"type": "string"},
                      "description": "The clips' ids, in any order."}
        }, "required": ["clips"]})
    },
}];

/// What the selection glues.
enum Pick {
    /// Audio clips, by key, per track.
    Audio(Vec<(usize, Vec<String>)>),
    /// Note clips, by id, on a track.
    Notes(usize, Vec<String>),
}

impl App {
    /// Runs the tool here, or `None` if `name` isn't it.
    pub(crate) fn glue_tool(&mut self, name: &str, args: &Value) -> Option<Done> {
        (name == "glue_clips").then(|| self.glue_clips_tool(args))
    }

    fn glue_clips_tool(&mut self, args: &Value) -> Done {
        let ids: Vec<String> = (args["clips"].as_array())
            .ok_or("Give the clips' ids as a list.")?
            .iter()
            .map(|v| v.as_str().map(String::from).ok_or("Clip ids are text."))
            .collect::<Result<_, _>>()?;
        if ids.len() < 2 {
            return Err("Give two clips or more on one track to glue.".into());
        }
        if !self.may_edit_song() {
            return Err("This project can be commented on, not changed.".into());
        }
        let audio =
            |id: &String| (self.tracks.iter()).position(|t| t.clips.iter().any(|c| &c.key == id));
        let notes = |id: &String| {
            (self.tracks.iter()).position(|t| t.note_clips.iter().any(|c| &c.id == id))
        };
        let mut on = None;
        let mut kinds = (false, false);
        for id in &ids {
            let i = match (audio(id), notes(id)) {
                (Some(i), _) => {
                    kinds.0 = true;
                    i
                }
                (None, Some(i)) => {
                    kinds.1 = true;
                    i
                }
                (None, None) => return Err(format!("There is no clip {id}.")),
            };
            if on.is_some_and(|t| t != i) {
                return Err(
                    "Those clips are on different tracks: glue the clips of one track.".into(),
                );
            }
            on = Some(i);
        }
        let i = on.ok_or("Give the clips' ids.")?;
        if kinds.0 && kinds.1 {
            return Err("Glue audio clips or note clips, not both together.".into());
        }
        if kinds.1 {
            let id = self.glue_notes(i, &ids)?;
            return Ok((json!({"clip": id, "glued": ids}), Some("Glue clips".into())));
        }
        let busy = self.renders.busy();
        let id = self.glue_audio(i, &ids)?;
        Ok((
            json!({"clip": id, "glued": ids, "rendering": self.renders.busy().max(busy)}),
            None,
        ))
    }

    /// Glues note clips `ids` of track `i`; gives the new clip's id.
    pub(crate) fn glue_notes(&mut self, i: usize, ids: &[String]) -> Result<String, String> {
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let v = self.edit_model(|p| {
            let t = p.tracks.get_mut(i).ok_or("That track is gone.")?;
            daw_project::glue_note_clips(&mut t.note_clips, &refs).map(Value::String)
        })?;
        let id = v.as_str().unwrap_or_default().to_string();
        self.timeline.note_sel = Some(id.clone());
        Ok(id)
    }

    /// Starts gluing audio clips `keys` of track `i`; gives the key the
    /// new clip will have.
    pub(crate) fn glue_audio(&mut self, i: usize, keys: &[String]) -> Result<String, String> {
        let t = self.tracks.get(i).ok_or("That track is gone.")?;
        let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        let order = daw_project::glue_order(
            &t.clips,
            &refs,
            |c: &Clip| c.key.as_str(),
            |c| (c.start, c.end()),
            |c| c.muted,
            |c| format!("\"{}\"", c.name()),
        )?;
        let clips: Vec<Clip> = order.iter().map(|&j| t.clips[j].clone()).collect();
        let key = daw_project::new_id();
        self.start_glue(i, clips, key.clone())?;
        Ok(key)
    }

    /// What the `glue` command glues now, or why nothing.
    fn glue_pick(&self) -> Result<Pick, String> {
        let sel = &self.timeline.selected;
        let mut audio: Vec<(usize, Vec<String>)> = (self.tracks.iter().enumerate())
            .map(|(i, t)| {
                let keys = (t.clips.iter())
                    .filter(|c| sel.contains(&c.id) && !c.muted)
                    .map(|c| c.key.clone())
                    .collect();
                (i, keys)
            })
            .filter(|(_, k): &(usize, Vec<String>)| !k.is_empty())
            .collect();
        // One clip: glued to the next one on its track.
        if let [(i, keys)] = audio.as_mut_slice()
            && let [key] = keys.as_slice()
        {
            let clips = &self.tracks[*i].clips;
            let at = clips.iter().find(|c| &c.key == key).map_or(0, |c| c.start);
            let next = (clips.iter())
                .filter(|c| !c.muted && &c.key != key && c.start >= at)
                .min_by_key(|c| c.start)
                .ok_or("There is no clip after it on its track to glue it to.")?;
            keys.push(next.key.clone());
        }
        audio.retain(|(_, k)| k.len() > 1);
        if !audio.is_empty() {
            return Ok(Pick::Audio(audio));
        }
        if let Some(id) = self.timeline.note_sel.as_deref() {
            for (i, t) in self.tracks.iter().enumerate() {
                let Some(c) = t.note_clips.iter().find(|c| c.id == id) else {
                    continue;
                };
                let next = (t.note_clips.iter())
                    .filter(|n| !n.muted && n.id != id && n.start >= c.start)
                    .min_by_key(|n| n.start)
                    .ok_or("There is no clip after it on its track to glue it to.")?;
                return Ok(Pick::Notes(i, vec![c.id.clone(), next.id.clone()]));
            }
        }
        Err(
            "Select the clips to glue first: two or more on a track, or one to glue to the next."
                .into(),
        )
    }

    /// Whether the `glue` command has anything to glue.
    pub(crate) fn can_glue(&self) -> bool {
        self.glue_pick().is_ok()
    }

    /// Glues the selected clips (the `glue` command).
    pub(crate) fn glue_selection(&mut self) {
        let done = match self.glue_pick() {
            Err(e) => Err(e),
            Ok(Pick::Notes(i, ids)) => self.glue_notes(i, &ids).map(|_| ()),
            Ok(Pick::Audio(groups)) => groups
                .into_iter()
                .try_for_each(|(i, keys)| self.glue_audio(i, &keys).map(|_| ())),
        };
        if let Err(e) = done {
            self.message = Some(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::{Value, json};

    use crate::Track;
    use crate::agent::driver::Driver;

    fn id(v: &Value, key: &str) -> String {
        v[key].as_str().unwrap().to_string()
    }

    /// Waits for the renders under way to come in.
    fn wait(d: &mut Driver) {
        for _ in 0..1000 {
            if d.app().renders.busy() == 0 {
                d.project();
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
            d.project();
        }
        panic!("timed out");
    }

    #[test]
    fn an_assistant_glues_note_clips_into_one_and_undo_takes_it_back() {
        let mut d = Driver::new();
        let t = id(
            &d.call("add_track", json!({"kind": "instrument"})).unwrap(),
            "track",
        );
        let clip = |d: &mut Driver, bar: i64, pitch: i64| {
            let v = d
                .call(
                    "add_note_clip",
                    json!({"track": t, "bar": bar, "bars": 1, "name": format!("Riff {bar}"),
                           "notes": [{"pitch": pitch, "beat": 0, "beats": 1}]}),
                )
                .unwrap();
            id(&v, "clip")
        };
        let a = clip(&mut d, 1, 60);
        let b = clip(&mut d, 2, 62);
        let c = clip(&mut d, 3, 64);
        let e = d.call("glue_clips", json!({"clips": [a, c]})).unwrap_err();
        assert!(e.contains("sits between"), "{e}");
        let v = d.call("glue_clips", json!({"clips": [b, a, c]})).unwrap();
        let g = id(&v, "clip");
        let p = d.project();
        let clips = p["tracks"][0]["note_clips"].as_array().unwrap();
        assert_eq!(clips.len(), 1, "{p}");
        assert_eq!(clips[0]["id"], g.as_str());
        assert_eq!(clips[0]["name"], "Riff 1");
        let notes = d.call("get_note_clip", json!({"clip": g})).unwrap();
        let pitches: Vec<i64> = (notes["notes"].as_array().unwrap().iter())
            .map(|n| n["pitch"].as_i64().unwrap())
            .collect();
        assert_eq!(pitches, [60, 62, 64]);
        let h = d.call("get_history", json!({})).unwrap();
        assert!(h.to_string().contains("Glue clips"), "{h}");
        d.command("undo");
        let p = d.project();
        assert_eq!(p["tracks"][0]["note_clips"].as_array().unwrap().len(), 3);

        // The command glues the selected clip to the next one.
        d.app().timeline.note_sel = Some(b.clone());
        d.command("glue");
        let p = d.project();
        let clips = p["tracks"][0]["note_clips"].as_array().unwrap();
        assert_eq!(clips.len(), 2, "{p}");
        assert_eq!(clips[1]["name"], "Riff 2");
        assert!(d.call("glue_clips", json!({"clips": [a]})).is_err());
        assert!(d.call("glue_clips", json!({"clips": [a, "nope"]})).is_err());
    }

    #[test]
    fn an_assistant_glues_audio_clips_into_one_that_sounds_the_same() {
        let mut d = Driver::new();
        let rate = d.app().rate as i64;
        // Two halves of a take, the second louder and faded out, with a
        // gap between them, on a track with a delay.
        let mut first =
            crate::timing::tests::take(rate as usize, &[100, 4000, 13_000, 18_000, 22_000]);
        let mut second = first.clone();
        first.len = rate as usize / 4;
        second.id = crate::timeline::next_id();
        second.key = daw_project::new_id();
        second.offset = rate as usize / 4;
        second.len = rate as usize / 4;
        second.start = rate / 2;
        second.gain_db = 3.0;
        second.fade_out.len = rate as usize / 8;
        let (a, b) = (first.key.clone(), second.key.clone());
        let app = d.app();
        let mut track = Track::new("Guitar".into());
        track.clips = vec![first, second];
        app.tracks.push(track);
        app.rebuild();
        let t = d.project()["tracks"][0]["id"].as_str().unwrap().to_string();
        d.call("add_effect", json!({"track": t, "effect": "delay"}))
            .unwrap();
        let frames = 2 * rate as usize;
        let before = crate::render::tests::render(d.app(), frames);
        assert!(before.iter().any(|x| x.abs() > 0.1), "it is heard");

        let v = d.call("glue_clips", json!({"clips": [b, a]})).unwrap();
        assert_eq!(v["rendering"], 1);
        let g = id(&v, "clip");
        wait(&mut d);
        let p = d.project();
        let clips = p["tracks"][0]["clips"].as_array().unwrap();
        assert_eq!(clips.len(), 1, "{p}");
        assert_eq!(clips[0]["id"], g.as_str());
        let app = d.app();
        assert_eq!(app.message, None);
        let c = &app.tracks[0].clips[0];
        assert_eq!(c.start, 0);
        assert_eq!(c.len, 3 * rate as usize / 4, "to the second one's end");
        assert_eq!(c.source.channels, 1, "mono stays mono");
        assert_eq!(app.tracks[0].fx.len(), 1, "the effects stay on the track");
        let after = crate::render::tests::render(d.app(), frames);
        let worst = crate::render::tests::worst(&before, &after);
        assert!(worst < 1e-4, "{worst}");

        d.command("undo");
        let p = d.project();
        assert_eq!(p["tracks"][0]["clips"].as_array().unwrap().len(), 2);
        let e = d.call("glue_clips", json!({"clips": [a, t]})).unwrap_err();
        assert!(e.contains("no clip"), "{e}");

        // The command glues the one selected to the next.
        let first = d.app().tracks[0].clips[0].id;
        d.app().timeline.selected = [first].into();
        d.command("glue");
        wait(&mut d);
        let p = d.project();
        assert_eq!(p["tracks"][0]["clips"].as_array().unwrap().len(), 1);
        let after = crate::render::tests::render(d.app(), frames);
        assert!(crate::render::tests::worst(&before, &after) < 1e-4);
    }
}
