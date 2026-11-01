// SPDX-License-Identifier: GPL-3.0-or-later
//! Assistants' tools for pitch editing (plan B2, roadmap 2.11): reading
//! the notes found in an audio clip, with their pitch line, and editing
//! them one by one or together, as the pitch editor pane does.

use serde_json::{Value, json};

use daw_project::{MAX_VIBRATO_PCT, PitchNote};

use crate::App;
use crate::agent::{Access, Tool};
use crate::pitch_edit::{MIN_SECONDS, SnapTo, clamp_cents, note_name, retime, snap_shift};
use crate::timeline::Clip;
use daw_engine::time::TempoMap;

/// What the tools here answer: a value, and the undo step's name.
type Done = Result<(Value, Option<String>), String>;

pub(crate) const TOOLS: &[Tool] = &[
    Tool {
        name: "get_pitch_notes",
        description: "The notes found in an audio clip for pitch editing (one voice or a \
                      single-note line), in order: each one's index, where it starts and \
                      how long it is (start_seconds and seconds: seconds of the file's own \
                      audio, from where the clip starts in it; timeline_seconds and \
                      timeline_end_seconds: where it is heard on the timeline, after any \
                      follow-tempo stretch and its own move), the pitch it was sung at in \
                      the file (midi with cents as the fraction, and its name), and its \
                      edits: shift_semitones, moved_seconds, resized_seconds, drift_pct and \
                      vibrato_pct (how much of each is kept; 100 as sung) and \
                      formant_semitones. 'now' names the note's pitch in the file with its \
                      shift, before the clip's transpose; 'plays' and plays_midi are what \
                      is heard, with the clip's transpose_semitones added (the same as \
                      'now' when the clip isn't transposed). With line: true, \
                      each note also gives its pitch line as sung, a midi number every 50 \
                      ms (null where there was no clear pitch). found is false until \
                      edit_pitch_notes with find: true has run.",
        access: Access::Read,
        schema: || {
            json!({"type": "object", "required": ["clip"], "properties": {
                "clip": {"type": "string", "description": "An audio clip's id."},
                "line": {"type": "boolean"}
            }})
        },
    },
    Tool {
        name: "edit_pitch_notes",
        description: "Edits an audio clip's notes one by one, as the pitch editor does; the \
                      clip plays the edits at once, through the stretcher, and nothing is \
                      rendered. find: true finds its notes first (again keeps the pitch \
                      edits of the notes the new ones replace). notes picks them by index \
                      (all when left out). Then, for each: to_midi puts it on that pitch \
                      in the file, as midi and 'now' read (fractions for cents), \
                      to_plays_midi so that it is heard at that pitch, with the clip's \
                      transpose (as plays_midi reads), shift_semitones sets its move, \
                      by_semitones adds to it, snap: true puts it on the nearest whole \
                      note and snap: \"key\" on the nearest note of the song's key \
                      (both as heard, with the clip's transpose), and min_cents leaves \
                      the notes already that close to where they would go (so snap: \
                      \"key\", min_cents: 30 snaps every note more than 30 cents off); \
                      move_seconds and \
                      resize_seconds, in the file's own seconds, set how far it is moved \
                      in time and made longer (they stop where the audio around it would \
                      stretch more than 3×); drift_pct (0 to 100) and vibrato_pct (0 to 300) set how \
                      much of each is kept; formant_semitones (-12 to 12) moves the \
                      voice's character; reset: true takes every edit off first. Answers \
                      with the notes it changed, as get_pitch_notes gives them, and count \
                      (how many the clip has); with find: true or all: true, with every \
                      note; and with a snap how many notes it moved (snapped) and to which \
                      key. One undo step.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "required": ["clip"], "properties": {
                "clip": {"type": "string", "description": "An audio clip's id."},
                "find": {"type": "boolean"},
                "notes": {"type": "array", "items": {"type": "integer"},
                          "description": "Indexes from get_pitch_notes, from 0."},
                "reset": {"type": "boolean"},
                "to_midi": {"type": "number", "description": "The pitch in the file, before the clip's transpose."},
                "to_plays_midi": {"type": "number", "description": "The pitch heard, after the clip's transpose."},
                "shift_semitones": {"type": "number", "description": "-12 to 12."},
                "by_semitones": {"type": "number"},
                "snap": {"enum": [true, false, "semitone", "key"],
                         "description": "true or \"semitone\": the nearest whole note; \
                                         \"key\": the nearest note of the song's key."},
                "min_cents": {"type": "number",
                              "description": "0 to 100: with snap, leave notes this close."},
                "all": {"type": "boolean", "description": "Answer with every note, not only those changed."},
                "move_seconds": {"type": "number"},
                "resize_seconds": {"type": "number"},
                "drift_pct": {"type": "number"},
                "vibrato_pct": {"type": "number"},
                "formant_semitones": {"type": "number", "description": "-12 to 12."}
            }})
        },
    },
];

/// A number from `v`, which must lie in `lo..=hi`.
fn within(v: &Value, what: &str, lo: f64, hi: f64) -> Result<Option<f64>, String> {
    match v {
        Value::Null => Ok(None),
        v => match v.as_f64() {
            Some(x) if (lo..=hi).contains(&x) => Ok(Some(x)),
            Some(_) => Err(format!("{what} goes from {lo} to {hi}.")),
            None => Err(format!("{what} is a number.")),
        },
    }
}

/// The notes of `c` as an assistant reads them.
pub(crate) fn notes_value(c: &Clip, map: &TempoMap, with_line: bool) -> Value {
    let rate = c.source.rate.max(1) as f64;
    let secs = |f: i64| ((f as f64 / rate) * 1000.0).round() / 1000.0;
    let out_rate = map.sample_rate().max(1) as f64;
    // Where source frame `f` is heard, in timeline seconds.
    let heard =
        |f: i64| ((c.out_of_frame(map, f as f64) as f64 / out_rate) * 1000.0).round() / 1000.0;
    let clip_cents = c.cents as i64;
    let notes: Vec<Value> = (c.pitch_notes.iter().enumerate())
        .map(|(i, n)| {
            let mut v = json!({
                "index": i,
                "start_seconds": secs(n.start - c.offset as i64),
                "seconds": secs(n.end - n.start),
                "midi": n.cents as f64 / 100.0,
                "pitch": note_name(n.cents),
                "now": note_name(n.cents + n.shift),
                "plays": note_name(n.cents + n.shift + clip_cents),
                "plays_midi": (n.cents + n.shift + clip_cents) as f64 / 100.0,
                "timeline_seconds": heard(n.start + n.moved),
                "timeline_end_seconds": heard(n.end + n.moved + n.resized),
                "shift_semitones": n.shift as f64 / 100.0,
                "moved_seconds": secs(n.moved),
                "resized_seconds": secs(n.resized),
                "drift_pct": n.drift,
                "vibrato_pct": n.vibrato,
                "formant_semitones": n.formant as f64 / 100.0,
            });
            if with_line && let Some(l) = c.line() {
                let step = (rate * 0.05) as i64;
                let mut f = n.start;
                let mut pts = Vec::new();
                while f < n.end {
                    pts.push(match l.get(l.hop_of(f)) {
                        Some(m) => json!((m as f64 * 100.0).round() / 100.0),
                        None => Value::Null,
                    });
                    f += step.max(1);
                }
                v["line"] = Value::Array(pts);
            }
            v
        })
        .collect();
    json!({"clip": c.key, "found": !c.pitch_notes.is_empty(),
           "transpose_semitones": clip_cents as f64 / 100.0, "notes": notes})
}

impl App {
    /// Runs one of the editing tools here, or `None` if `name` isn't one.
    pub(crate) fn pitch_tool(&mut self, name: &str, args: &Value) -> Option<Done> {
        (name == "edit_pitch_notes").then(|| self.edit_pitch_notes(args))
    }

    /// Answers one of the looking tools here, or `None` if `name` isn't one.
    pub(crate) fn pitch_read(&mut self, name: &str, args: &Value) -> Option<Result<Value, String>> {
        (name == "get_pitch_notes").then(|| {
            let (l, k) = self.pitch_audio_clip(args)?;
            let c = &self.tracks[l].clips[k];
            if args["line"].as_bool() == Some(true) && !c.pitch_notes.is_empty() {
                let id = c.id;
                self.ensure_line_now(id)?;
            }
            Ok(notes_value(
                &self.tracks[l].clips[k],
                &self.tempo_map(),
                args["line"].as_bool() == Some(true),
            ))
        })
    }

    fn pitch_audio_clip(&self, args: &Value) -> Result<(usize, usize), String> {
        let key = args["clip"].as_str().ok_or("Give the clip's id.")?;
        for (i, t) in self.tracks.iter().enumerate() {
            if let Some(j) = t.clips.iter().position(|c| c.key == key) {
                return Ok((i, j));
            }
        }
        if (self.tracks.iter().flat_map(|t| &t.note_clips)).any(|c| c.id == key) {
            return Err(format!(
                "{key} is a note clip; pitch editing works on audio clips (edit_notes moves \
                 a note clip's notes)."
            ));
        }
        Err(format!("There is no clip {key}."))
    }

    fn edit_pitch_notes(&mut self, args: &Value) -> Done {
        if !self.may_edit_song() {
            return Err("This project's role here can only comment.".into());
        }
        let (l, k) = self.pitch_audio_clip(args)?;
        // Check everything before changing anything.
        let to_midi = within(&args["to_midi"], "to_midi", 0.0, 127.0)?;
        let to_plays = within(&args["to_plays_midi"], "to_plays_midi", 0.0, 127.0)?;
        if to_midi.is_some() && to_plays.is_some() {
            return Err("Give to_midi or to_plays_midi, not both.".into());
        }
        let shift = within(&args["shift_semitones"], "shift_semitones", -12.0, 12.0)?;
        let by = within(&args["by_semitones"], "by_semitones", -24.0, 24.0)?;
        let moved = within(&args["move_seconds"], "move_seconds", -60.0, 60.0)?;
        let resized = within(&args["resize_seconds"], "resize_seconds", -60.0, 60.0)?;
        let drift = within(&args["drift_pct"], "drift_pct", 0.0, 100.0)?;
        let vib = within(
            &args["vibrato_pct"],
            "vibrato_pct",
            0.0,
            MAX_VIBRATO_PCT as f64,
        )?;
        let formant = within(&args["formant_semitones"], "formant_semitones", -12.0, 12.0)?;
        let snap = match &args["snap"] {
            Value::Null | Value::Bool(false) => None,
            Value::Bool(true) => Some(SnapTo::Semitone),
            Value::String(s) if s == "semitone" => Some(SnapTo::Semitone),
            Value::String(s) if s == "key" => Some(SnapTo::Key(self.scale.ok_or(
                "The song has no key to snap to: set one with set_key, or snap: true for \
                 the nearest whole note.",
            )?)),
            _ => return Err("snap is true, \"semitone\" or \"key\".".into()),
        };
        let min_cents = within(&args["min_cents"], "min_cents", 0.0, 100.0)?;
        if min_cents.is_some() && snap.is_none() {
            return Err("min_cents goes with snap.".into());
        }
        let min_cents = min_cents.map_or(0, |m| m.round() as i64);
        let reset = args["reset"].as_bool() == Some(true);
        let picked: Option<Vec<usize>> = match &args["notes"] {
            Value::Null => None,
            Value::Array(a) => Some(
                a.iter()
                    .map(|v| v.as_u64().map(|i| i as usize))
                    .collect::<Option<_>>()
                    .ok_or("notes is a list of indexes.")?,
            ),
            _ => return Err("notes is a list of indexes.".into()),
        };
        let id = self.tracks[l].clips[k].id;
        let before = self.tracks[l].clips[k].pitch_notes.clone();
        if args["find"].as_bool() == Some(true) {
            self.find_notes_now(id)?;
        }
        let c = &self.tracks[l].clips[k];
        if c.pitch_notes.is_empty() {
            return Err(if args["find"].as_bool() == Some(true) {
                "No notes were found in that clip: pitch editing reads one voice or a \
                 single-note line, with a clear pitch."
                    .into()
            } else {
                "That clip's notes haven't been found yet: call with find: true.".into()
            });
        }
        let count = c.pitch_notes.len();
        let targets: Vec<usize> = match picked {
            None => (0..count).collect(),
            Some(p) => {
                if let Some(bad) = p.iter().find(|&&i| i >= count) {
                    return Err(format!(
                        "There is no note {bad}; the clip has {count}, from 0."
                    ));
                }
                p
            }
        };
        if drift.is_some() || vib.is_some() {
            self.ensure_line_now(id)?;
        }
        let rate = self.tracks[l].clips[k].source.rate as f64;
        let frames = |s: f64| (s * rate).round() as i64;
        let c = &mut self.tracks[l].clips[k];
        let clip_cents = c.cents as i64;
        let mut snapped = 0;
        let range = (c.offset as i64, (c.offset + c.len) as i64);
        let min_len = frames(MIN_SECONDS);
        for &i in &targets {
            let n = &mut c.pitch_notes[i];
            if reset {
                *n = PitchNote::found(n.start, n.end, n.cents);
            }
            if let Some(m) = to_midi {
                n.shift = clamp_cents((m * 100.0).round() as i64 - n.cents);
            }
            if let Some(m) = to_plays {
                n.shift = clamp_cents((m * 100.0).round() as i64 - clip_cents - n.cents);
            }
            if let Some(s) = shift {
                n.shift = clamp_cents((s * 100.0).round() as i64);
            }
            if let Some(b) = by {
                n.shift = clamp_cents(n.shift + (b * 100.0).round() as i64);
            }
            if let Some(to) = snap
                && let Some(s) = snap_shift(n.cents, n.shift, clip_cents, to, min_cents)
            {
                snapped += usize::from(s != n.shift);
                n.shift = s;
            }
            if let Some(d) = drift {
                n.drift = d.round() as i64;
            }
            if let Some(v) = vib {
                n.vibrato = v.round() as i64;
            }
            if let Some(f) = formant {
                n.formant = clamp_cents((f * 100.0).round() as i64);
            }
        }
        if moved.is_some() || resized.is_some() {
            for &i in &targets {
                let n = c.pitch_notes[i];
                retime(
                    &mut c.pitch_notes,
                    i,
                    moved.map_or(n.moved, frames),
                    resized.map_or(n.resized, frames),
                    range,
                    min_len,
                );
            }
        }
        let changed = c.pitch_notes != before;
        if changed {
            self.rebuild();
        }
        let found = args["find"].as_bool() == Some(true);
        let c = &self.tracks[l].clips[k];
        let mut v = notes_value(c, &self.tempo_map(), false);
        let count = c.pitch_notes.len();
        if !found && args["all"].as_bool() != Some(true) {
            // Only the notes that differ from before: a neighbour moved
            // by a retime counts.
            let after = &c.pitch_notes;
            let differs = |i: usize| before.get(i) != after.get(i);
            if let Some(list) = v["notes"].as_array_mut() {
                let mut i = 0;
                list.retain(|_| {
                    i += 1;
                    differs(i - 1)
                });
            }
        }
        v["count"] = json!(count);
        if let Some(to) = snap {
            v["snapped"] = json!(snapped);
            if let SnapTo::Key(s) = to {
                v["key"] = json!(s.label());
            }
        }
        Ok((v, changed.then(|| "Edit pitch notes".to_string())))
    }
}
