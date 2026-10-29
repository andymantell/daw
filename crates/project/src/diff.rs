// SPDX-License-Identifier: GPL-3.0-or-later
//! What changed between two versions of a project (roadmap 2.8), in
//! words: "Tempo 96 → 120 BPM", "Added track "Bass"", "Moved "Solo" from
//! bar 9 to bar 11". Used to show what a snapshot would bring back, or
//! what changed from one snapshot to the next.
//!
//! Things are matched by id, so a renamed track is one track renamed,
//! not one removed and one added. Comments aren't compared: a restore
//! leaves them as they are (see `snapshots.rs`), and nor is frozen
//! audio, which follows from the clips and effects.

use std::collections::{HashMap, HashSet};

use crate::{Clip, Effect, Fade, Project, Track};

/// One thing that changed, and where: "Song", a track's name,
/// "Tracks", "Markers", "Lesson notes" or "Tab".
#[derive(Clone, Debug, PartialEq)]
pub struct Difference {
    pub place: String,
    pub text: String,
}

/// Clip positions may differ by this many frames and count as the same,
/// as converting a project to another rate and back rounds them.
const SLACK: i64 = 2;

/// What changed going from `before` to `after`, song first, then each
/// track in `after`'s order, then markers, notes and tab blocks.
pub fn differences(before: &Project, after: &Project) -> Vec<Difference> {
    let mut out = Vec::new();
    let mut say = |place: &str, text: String| {
        out.push(Difference {
            place: place.into(),
            text,
        })
    };

    // The song.
    if before.tempo_bpm != after.tempo_bpm {
        say(
            "Song",
            format!(
                "Tempo {} → {} BPM",
                number(before.tempo_bpm),
                number(after.tempo_bpm)
            ),
        );
    }
    if (before.beats_per_bar, before.beat_unit) != (after.beats_per_bar, after.beat_unit) {
        say(
            "Song",
            format!(
                "Time signature {}/{} → {}/{}",
                before.beats_per_bar, before.beat_unit, after.beats_per_bar, after.beat_unit
            ),
        );
    }
    // Tempo and time signature changes later in the song.
    let was: HashMap<&str, _> = (before.tempo_changes.iter())
        .map(|c| (c.id.as_str(), c))
        .collect();
    let now: HashSet<&str> = after.tempo_changes.iter().map(|c| c.id.as_str()).collect();
    let at = |bar: i64, beat: u32| match beat {
        1 => format!("bar {bar}"),
        b => format!("bar {bar} beat {b}"),
    };
    for c in &after.tempo_changes {
        match was.get(c.id.as_str()) {
            None => say(
                "Song",
                format!(
                    "Tempo change to {} BPM at {}",
                    number(c.bpm),
                    at(c.bar, c.beat)
                ),
            ),
            Some(b) if *b != c => say(
                "Song",
                format!(
                    "Tempo change at {} ({} BPM) → at {} ({} BPM)",
                    at(b.bar, b.beat),
                    number(b.bpm),
                    at(c.bar, c.beat),
                    number(c.bpm)
                ),
            ),
            Some(_) => {}
        }
    }
    for c in (before.tempo_changes.iter()).filter(|c| !now.contains(c.id.as_str())) {
        say(
            "Song",
            format!("Removed the tempo change at {}", at(c.bar, c.beat)),
        );
    }
    let was: HashMap<&str, _> = (before.meter_changes.iter())
        .map(|c| (c.id.as_str(), c))
        .collect();
    let now: HashSet<&str> = after.meter_changes.iter().map(|c| c.id.as_str()).collect();
    for c in &after.meter_changes {
        match was.get(c.id.as_str()) {
            None => say(
                "Song",
                format!(
                    "Time signature change to {}/{} at bar {}",
                    c.beats, c.unit, c.bar
                ),
            ),
            Some(b) if *b != c => say(
                "Song",
                format!(
                    "Time signature change at bar {} ({}/{}) → at bar {} ({}/{})",
                    b.bar, b.beats, b.unit, c.bar, c.beats, c.unit
                ),
            ),
            Some(_) => {}
        }
    }
    for c in (before.meter_changes.iter()).filter(|c| !now.contains(c.id.as_str())) {
        say(
            "Song",
            format!("Removed the time signature change at bar {}", c.bar),
        );
    }
    if before.loop_bars != after.loop_bars {
        let bars = |(a, b): (i64, i64)| format!("{a}–{b}");
        say(
            "Song",
            format!(
                "Loop bars {} → {}",
                bars(before.loop_bars),
                bars(after.loop_bars)
            ),
        );
    }
    if before.reference_hz != after.reference_hz {
        say(
            "Song",
            format!(
                "Tuning A = {} → {} Hz",
                number(before.reference_hz),
                number(after.reference_hz)
            ),
        );
    }

    // Tracks.
    let was: HashMap<&str, &Track> = before.tracks.iter().map(|t| (t.id.as_str(), t)).collect();
    let now: HashSet<&str> = after.tracks.iter().map(|t| t.id.as_str()).collect();
    for t in &after.tracks {
        match was.get(t.id.as_str()) {
            None => say("Tracks", format!("Added track “{}”", t.name)),
            Some(b) => {
                let k = after.rate.max(1) as f64 / before.rate.max(1) as f64;
                for text in track_changes(b, t, k) {
                    say(&t.name, text);
                }
            }
        }
    }
    for t in before
        .tracks
        .iter()
        .filter(|t| !now.contains(t.id.as_str()))
    {
        say("Tracks", format!("Removed track “{}”", t.name));
    }
    let order = |p: &Project, keep: &dyn Fn(&str) -> bool| -> Vec<String> {
        (p.tracks.iter())
            .filter(|t| keep(&t.id))
            .map(|t| t.id.clone())
            .collect()
    };
    let in_both = |id: &str| was.contains_key(id) && now.contains(id);
    if order(before, &in_both) != order(after, &in_both) {
        say("Tracks", "Changed the order of the tracks".into());
    }

    // Markers.
    let was: HashMap<&str, _> = before.markers.iter().map(|m| (m.id.as_str(), m)).collect();
    let now: HashSet<&str> = after.markers.iter().map(|m| m.id.as_str()).collect();
    for m in &after.markers {
        match was.get(m.id.as_str()) {
            None => say("Markers", format!("Added “{}” at bar {}", m.name, m.bar)),
            Some(b) => {
                if b.name != m.name {
                    say("Markers", format!("Renamed “{}” to “{}”", b.name, m.name));
                }
                if b.bar != m.bar {
                    say(
                        "Markers",
                        format!("Moved “{}” from bar {} to bar {}", m.name, b.bar, m.bar),
                    );
                }
            }
        }
    }
    for m in before
        .markers
        .iter()
        .filter(|m| !now.contains(m.id.as_str()))
    {
        say("Markers", format!("Removed “{}” (bar {})", m.name, m.bar));
    }

    // Lesson notes.
    if before.notes != after.notes {
        let text = if before.notes.trim().is_empty() {
            "Written".to_string()
        } else if after.notes.trim().is_empty() {
            "Cleared".to_string()
        } else {
            let (added, removed) = lines_changed(&before.notes, &after.notes);
            let mut parts = Vec::new();
            if added > 0 {
                parts.push(count(added, "line added", "lines added"));
            }
            if removed > 0 {
                parts.push(count(removed, "line removed", "lines removed"));
            }
            if parts.is_empty() {
                "Edited".to_string()
            } else {
                format!("Edited: {}", parts.join(", "))
            }
        };
        say("Lesson notes", text);
    }

    // Tab blocks.
    let section = |p: &Project, marker: &Option<String>| match marker {
        None => "the whole song".to_string(),
        Some(id) => (p.markers.iter())
            .find(|m| &m.id == id)
            .map_or("no section".into(), |m| format!("“{}”", m.name)),
    };
    let was: HashMap<&str, _> = before.tabs.iter().map(|b| (b.id.as_str(), b)).collect();
    let now: HashSet<&str> = after.tabs.iter().map(|b| b.id.as_str()).collect();
    for b in &after.tabs {
        match was.get(b.id.as_str()) {
            None => say("Tab", format!("Added “{}”", title(&b.title))),
            Some(o) => {
                if o.title != b.title {
                    say(
                        "Tab",
                        format!("Renamed “{}” to “{}”", title(&o.title), title(&b.title)),
                    );
                }
                if o.text != b.text {
                    say("Tab", format!("Edited “{}”", title(&b.title)));
                }
                if o.marker != b.marker {
                    say(
                        "Tab",
                        format!(
                            "Pinned “{}” to {}",
                            title(&b.title),
                            section(after, &b.marker)
                        ),
                    );
                }
            }
        }
    }
    for b in before.tabs.iter().filter(|b| !now.contains(b.id.as_str())) {
        say("Tab", format!("Removed “{}”", title(&b.title)));
    }
    out
}

/// What changed on one track. `k` turns `before`'s frame counts into
/// `after`'s, should their rates differ.
fn track_changes(before: &Track, after: &Track, k: f64) -> Vec<String> {
    let mut out = Vec::new();
    if before.name != after.name {
        out.push(format!("Renamed from “{}”", before.name));
    }
    if before.gain_db != after.gain_db {
        out.push(format!(
            "Volume {} → {}",
            db(before.gain_db),
            db(after.gain_db)
        ));
    }
    if before.pan != after.pan {
        out.push(format!("Pan {} → {}", pan(before.pan), pan(after.pan)));
    }
    if before.mute != after.mute {
        out.push(if after.mute { "Muted" } else { "Unmuted" }.into());
    }
    if before.solo != after.solo {
        out.push(
            if after.solo {
                "Soloed"
            } else {
                "No longer soloed"
            }
            .into(),
        );
    }

    // Clips.
    let was: HashMap<&str, &Clip> = before.clips.iter().map(|c| (c.id.as_str(), c)).collect();
    let now: HashSet<&str> = after.clips.iter().map(|c| c.id.as_str()).collect();
    let scale = |v: i64| (v as f64 * k).round() as i64;
    let near = |a: i64, b: i64| (scale(a) - b).abs() <= SLACK;
    let (mut added, mut changed, mut level, mut muted, mut unmuted) =
        (vec![], vec![], vec![], vec![], vec![]);
    for c in &after.clips {
        match was.get(c.id.as_str()) {
            None => added.push(c),
            Some(b) => {
                if b.asset != c.asset
                    || !near(b.start, c.start)
                    || !near(b.offset, c.offset)
                    || !near(b.len, c.len)
                {
                    changed.push(c);
                }
                let fades_near = |x: &Fade, y: &Fade| x.curve == y.curve && near(x.len, y.len);
                let envelope_near = b.envelope.len() == c.envelope.len()
                    && (b.envelope.iter().zip(&c.envelope))
                        .all(|(x, y)| x.db == y.db && near(x.at, y.at));
                if b.gain_db != c.gain_db
                    || !fades_near(&b.fade_in, &c.fade_in)
                    || !fades_near(&b.fade_out, &c.fade_out)
                    || !envelope_near
                {
                    level.push(c);
                }
                if b.muted != c.muted {
                    if c.muted { &mut muted } else { &mut unmuted }.push(c);
                }
            }
        }
    }
    let removed: Vec<&Clip> = (before.clips.iter())
        .filter(|c| !now.contains(c.id.as_str()))
        .collect();
    for (clips, one, many) in [
        (&added, "Added clip", "clips added"),
        (&removed, "Removed clip", "clips removed"),
        (&changed, "Moved or trimmed clip", "clips moved or trimmed"),
        (
            &level,
            "Changed the volume or fades of clip",
            "clips' volume or fades changed",
        ),
        (&muted, "Muted clip", "clips muted"),
        (&unmuted, "Unmuted clip", "clips unmuted"),
    ] {
        match clips.as_slice() {
            [] => {}
            [c] => out.push(format!("{one} “{}”", c.name)),
            all => out.push(format!("{} {many}", all.len())),
        }
    }

    // Effects.
    let was: HashMap<&str, &Effect> = before.effects.iter().map(|e| (e.id.as_str(), e)).collect();
    let now: HashSet<&str> = after.effects.iter().map(|e| e.id.as_str()).collect();
    for e in &after.effects {
        match was.get(e.id.as_str()) {
            None => out.push(format!("Added effect “{}”", e.name)),
            Some(b) => {
                if b.bypass != e.bypass {
                    let on = if e.bypass {
                        "Switched off"
                    } else {
                        "Switched on"
                    };
                    out.push(format!("{on} “{}”", e.name));
                }
                // Settings not read (`None`) on either side can't be told
                // apart, so count as unchanged.
                let plugin_changed = matches!((&b.state, &e.state), (Some(x), Some(y)) if x != y);
                if plugin_changed || b.params != e.params {
                    out.push(format!("Changed the settings of “{}”", e.name));
                }
            }
        }
    }
    for e in before
        .effects
        .iter()
        .filter(|e| !now.contains(e.id.as_str()))
    {
        out.push(format!("Removed effect “{}”", e.name));
    }
    let order = |t: &Track| -> Vec<String> {
        (t.effects.iter())
            .map(|e| e.id.clone())
            .filter(|id| was.contains_key(id.as_str()) && now.contains(id.as_str()))
            .collect()
    };
    if order(before) != order(after) {
        out.push("Changed the order of the effects".into());
    }
    out
}

/// How many lines `after` has that `before` doesn't, and the other way
/// round, matching lines in order (longest common subsequence). Very
/// long notes are compared as sets of lines instead.
fn lines_changed(before: &str, after: &str) -> (usize, usize) {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    let common = if a.len() * b.len() <= 4_000_000 {
        let mut row = vec![0usize; b.len() + 1];
        for x in &a {
            let mut diag = 0;
            for (j, y) in b.iter().enumerate() {
                let up = row[j + 1];
                row[j + 1] = if x == y { diag + 1 } else { up.max(row[j]) };
                diag = up;
            }
        }
        row[b.len()]
    } else {
        let set: HashSet<&str> = a.iter().copied().collect();
        b.iter().filter(|l| set.contains(*l)).count().min(a.len())
    };
    (b.len() - common, a.len() - common)
}

fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// A number without a needless ".0": 120, 96.5.
fn number(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn db(v: f64) -> String {
    format!("{v:+.1} dB").replace('-', "−")
}

fn pan(v: f64) -> String {
    let pct = (v * 100.0).round() as i64;
    match pct {
        0 => "centre".into(),
        p if p < 0 => format!("{}% left", -p),
        p => format!("{p}% right"),
    }
}

fn title(t: &str) -> &str {
    if t.trim().is_empty() { "Untitled" } else { t }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectState, Marker, TabBlock};

    fn clip(id: &str, start: i64) -> Clip {
        Clip {
            id: id.into(),
            asset: "a.wav".into(),
            name: format!("take {id}"),
            start,
            offset: 0,
            len: 100,
            take: None,
            muted: false,
            seq: 0,
            ..Clip::default()
        }
    }

    fn effect(id: &str, name: &str, state: u8) -> Effect {
        Effect {
            id: id.into(),
            name: name.into(),
            class: vec![1; 16],
            bundle: String::new(),
            bypass: false,
            state: Some(EffectState {
                component: vec![state],
                controller: None,
            }),
            ..Effect::default()
        }
    }

    fn texts(d: &[Difference]) -> Vec<String> {
        d.iter()
            .map(|d| format!("{}: {}", d.place, d.text))
            .collect()
    }

    fn song() -> Project {
        let mut p = Project::default();
        let mut g = Track::new("g".into(), "Guitar".into());
        g.clips = vec![clip("c1", 0), clip("c2", 500)];
        g.effects = vec![effect("e1", "Drive", 1), effect("e2", "Echo", 1)];
        p.tracks = vec![g, Track::new("b".into(), "Bass".into())];
        p.markers = vec![
            Marker {
                id: "m1".into(),
                bar: 1,
                name: "Intro".into(),
            },
            Marker {
                id: "m2".into(),
                bar: 9,
                name: "Solo".into(),
            },
        ];
        p.notes = "# Lesson\n- G chord\n- C chord".into();
        p.tabs = vec![TabBlock {
            id: "t1".into(),
            marker: Some("m2".into()),
            title: "Solo lick".into(),
            text: "e|--5--|".into(),
            author: String::new(),
            created: 1,
        }];
        p
    }

    #[test]
    fn the_same_project_has_no_differences() {
        assert!(differences(&song(), &song()).is_empty());
    }

    #[test]
    fn song_settings_read_as_before_and_after() {
        let mut q = song();
        q.tempo_bpm = 96.5;
        q.beats_per_bar = 6;
        q.beat_unit = 8;
        q.tempo_changes.push(crate::TempoChange {
            id: "tc".into(),
            bar: 9,
            beat: 3,
            bpm: 100.0,
            glide: false,
        });
        q.meter_changes.push(crate::MeterChange {
            id: "mc".into(),
            bar: 17,
            beats: 3,
            unit: 4,
        });
        q.loop_bars = (9, 13);
        q.reference_hz = 442.0;
        assert_eq!(
            texts(&differences(&song(), &q)),
            [
                "Song: Tempo 120 → 96.5 BPM",
                "Song: Time signature 4/4 → 6/8",
                "Song: Tempo change to 100 BPM at bar 9 beat 3",
                "Song: Time signature change to 3/4 at bar 17",
                "Song: Loop bars 1–5 → 9–13",
                "Song: Tuning A = 440 → 442 Hz",
            ]
        );
    }

    #[test]
    fn tracks_are_matched_by_id() {
        let mut q = song();
        q.tracks[0].name = "Lead".into();
        q.tracks[0].gain_db = -3.0;
        q.tracks[0].pan = -0.25;
        q.tracks[0].mute = true;
        q.tracks.remove(1);
        q.tracks.push(Track::new("v".into(), "Vocals".into()));
        assert_eq!(
            texts(&differences(&song(), &q)),
            [
                "Lead: Renamed from “Guitar”",
                "Lead: Volume +0.0 dB → −3.0 dB",
                "Lead: Pan centre → 25% left",
                "Lead: Muted",
                "Tracks: Added track “Vocals”",
                "Tracks: Removed track “Bass”",
            ]
        );
        let mut q = song();
        q.tracks.reverse();
        assert_eq!(
            texts(&differences(&song(), &q)),
            ["Tracks: Changed the order of the tracks"]
        );
    }

    #[test]
    fn clips_and_effects_are_summed_up() {
        let mut q = song();
        let g = &mut q.tracks[0];
        g.clips[0].start = 1000;
        g.clips[1].muted = true;
        g.clips[1].fade_in.len = 480;
        g.clips.push(clip("c3", 2000));
        g.clips.push(clip("c4", 3000));
        g.effects[0].bypass = true;
        g.effects[1].state = Some(EffectState {
            component: vec![2],
            controller: None,
        });
        g.effects.push(effect("e3", "Chorus", 1));
        assert_eq!(
            texts(&differences(&song(), &q)),
            [
                "Guitar: 2 clips added",
                "Guitar: Moved or trimmed clip “take c1”",
                "Guitar: Changed the volume or fades of clip “take c2”",
                "Guitar: Muted clip “take c2”",
                "Guitar: Switched off “Drive”",
                "Guitar: Changed the settings of “Echo”",
                "Guitar: Added effect “Chorus”",
            ]
        );
        let mut q = song();
        q.tracks[0].clips.clear();
        q.tracks[0].effects.swap(0, 1);
        q.tracks[0].effects[0].state = None;
        assert_eq!(
            texts(&differences(&song(), &q)),
            [
                "Guitar: 2 clips removed",
                "Guitar: Changed the order of the effects"
            ],
            "settings not read count as unchanged"
        );
    }

    #[test]
    fn a_built_in_effects_settings_are_compared() {
        let eq = |low: f64| Effect {
            id: "e4".into(),
            name: "EQ".into(),
            builtin: "eq".into(),
            params: vec![("low".into(), low)],
            ..Effect::default()
        };
        let mut p = song();
        p.tracks[1].effects.push(eq(0.0));
        let mut q = p.clone();
        q.tracks[1].effects[0] = eq(3.0);
        assert_eq!(
            texts(&differences(&p, &q)),
            ["Bass: Changed the settings of “EQ”"]
        );
    }

    #[test]
    fn a_different_rate_alone_is_no_change() {
        let mut q = song();
        q.rate = 44_100;
        for c in &mut q.tracks[0].clips {
            c.start = (c.start as f64 * 44_100.0 / 48_000.0).round() as i64;
            c.len = (c.len as f64 * 44_100.0 / 48_000.0).round() as i64;
        }
        assert!(differences(&song(), &q).is_empty());
    }

    #[test]
    fn markers_notes_and_tab_blocks() {
        let mut q = song();
        q.markers[0].name = "Count-in".into();
        q.markers[1].bar = 11;
        q.markers.push(Marker {
            id: "m3".into(),
            bar: 17,
            name: "Outro".into(),
        });
        q.notes = "# Lesson\n- G chord\n- D chord\n- E minor".into();
        q.tabs[0].text = "e|--7--|".into();
        q.tabs[0].marker = Some("m3".into());
        q.tabs.push(TabBlock {
            id: "t2".into(),
            marker: None,
            title: String::new(),
            text: "x".into(),
            author: String::new(),
            created: 2,
        });
        assert_eq!(
            texts(&differences(&song(), &q)),
            [
                "Markers: Renamed “Intro” to “Count-in”",
                "Markers: Moved “Solo” from bar 9 to bar 11",
                "Markers: Added “Outro” at bar 17",
                "Lesson notes: Edited: 2 lines added, 1 line removed",
                "Tab: Edited “Solo lick”",
                "Tab: Pinned “Solo lick” to “Outro”",
                "Tab: Added “Untitled”",
            ]
        );
        let mut r = song();
        r.markers.clear();
        r.notes.clear();
        r.tabs.clear();
        assert_eq!(
            texts(&differences(&song(), &r)),
            [
                "Markers: Removed “Intro” (bar 1)",
                "Markers: Removed “Solo” (bar 9)",
                "Lesson notes: Cleared",
                "Tab: Removed “Solo lick”",
            ]
        );
        assert_eq!(texts(&differences(&r, &song()))[2], "Lesson notes: Written");
    }

    #[test]
    fn line_counts() {
        assert_eq!(lines_changed("a\nb\nc", "a\nc\nd"), (1, 1));
        assert_eq!(lines_changed("", "a"), (1, 0));
        assert_eq!(lines_changed("a\nb", "b\na"), (1, 1));
    }
}
