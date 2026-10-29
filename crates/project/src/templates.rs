// SPDX-License-Identifier: GPL-3.0-or-later
//! Project templates (roadmap 2.8): a project to start from, such as
//! "Guitar lesson" with its tracks, sections and notes laid out, and
//! the user's own, saved from a project they have set up.
//!
//! A template is a [`Project`] without the things that belong to one
//! song: no audio (clips and frozen audio), no comments. Tracks keep
//! their names, mixer settings and effects (plugin settings included),
//! and the song keeps its tempo and time signature (with their changes),
//! loop, tuning, markers, lesson notes and tab blocks.
//!
//! A saved template is a project document holding just that, written by
//! a fresh document so no history comes with it. Starting a project from
//! one gives everything new ids, so two projects made from the same
//! template never share any.

use std::collections::HashMap;

use crate::{Marker, Project, ProjectDoc, Track, new_id, new_peer};

/// What saved templates' files are called: `Guitar lesson.dawtemplate`.
pub const TEMPLATE_EXTENSION: &str = "dawtemplate";

/// A project to start from.
#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    pub name: String,
    /// One line on what it sets up.
    pub about: String,
    pub project: Project,
}

impl Project {
    /// This project as a template: without its audio or comments.
    pub fn as_template(&self) -> Project {
        let mut p = self.clone();
        p.comments.clear();
        for t in &mut p.tracks {
            t.clips.clear();
            t.frozen = None;
        }
        p
    }

    /// A new project from this template, at `rate`, with new ids for
    /// everything in it. Tab blocks stay pinned to their sections.
    pub fn from_template(&self, rate: u32) -> Project {
        let mut p = self.as_template();
        p.rate = rate;
        for t in &mut p.tracks {
            t.id = new_id();
            for e in &mut t.effects {
                e.id = new_id();
            }
        }
        for c in &mut p.tempo_changes {
            c.id = new_id();
        }
        for c in &mut p.meter_changes {
            c.id = new_id();
        }
        let mut markers = HashMap::new();
        for m in &mut p.markers {
            let id = new_id();
            markers.insert(std::mem::replace(&mut m.id, id.clone()), id);
        }
        for b in &mut p.tabs {
            b.id = new_id();
            b.marker = b.marker.as_ref().and_then(|m| markers.get(m).cloned());
        }
        p
    }
}

/// A template as a file's bytes.
pub fn template_to_bytes(p: &Project) -> Vec<u8> {
    let mut doc = ProjectDoc::new(new_peer());
    doc.set_quietly(&p.as_template());
    doc.to_bytes()
}

/// A template from a file's bytes.
pub fn template_from_bytes(bytes: &[u8]) -> Result<Project, String> {
    Ok(ProjectDoc::from_bytes(bytes, new_peer())?
        .get()
        .as_template())
}

/// The templates that come with the app, empty project first.
pub fn built_in_templates() -> Vec<Template> {
    let track = |name: &str| Track::new(String::new(), name.into());
    let marker = |bar: i64, name: &str| Marker {
        id: name.to_lowercase(),
        bar,
        name: name.into(),
    };
    let empty = Project::default();
    let lesson = Project {
        tempo_bpm: 90.0,
        loop_bars: (5, 9),
        tracks: vec![track("Teacher"), track("Student"), track("Backing track")],
        markers: vec![
            marker(1, "Warm-up"),
            marker(5, "Song"),
            marker(21, "Try it yourself"),
        ],
        notes: "# This week\n- \n\n# To practise\n- \n".into(),
        ..Project::default()
    };
    let band = Project {
        tracks: vec![
            track("Drums"),
            track("Bass"),
            track("Rhythm guitar"),
            track("Lead guitar"),
            track("Vocals"),
        ],
        markers: [
            (1, "Intro"),
            (5, "Verse 1"),
            (13, "Chorus 1"),
            (21, "Verse 2"),
            (29, "Chorus 2"),
            (37, "Solo"),
            (45, "Chorus 3"),
            (53, "Outro"),
        ]
        .iter()
        .map(|&(bar, name)| marker(bar, name))
        .collect(),
        loop_bars: (13, 21),
        ..Project::default()
    };
    vec![
        Template {
            name: "Empty".into(),
            about: "No tracks, at 120 BPM in 4/4.".into(),
            project: empty,
        },
        Template {
            name: "Guitar lesson".into(),
            about: "Teacher, student and backing tracks, sections for the lesson, \
                    and notes to fill in."
                .into(),
            project: lesson,
        },
        Template {
            name: "Band demo".into(),
            about: "Drums, bass, two guitars and vocals, with verse and chorus sections.".into(),
            project: band,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Clip, Comment, Effect, EffectState, TabBlock};

    fn set_up() -> Project {
        let mut p = Project {
            tempo_bpm: 72.0,
            beats_per_bar: 3,
            notes: "# Warm-up".into(),
            ..Project::default()
        };
        let mut t = Track::new("t1".into(), "Guitar".into());
        t.gain_db = -6.0;
        t.clips.push(Clip {
            id: "c1".into(),
            asset: "aaaa.wav".into(),
            name: "take".into(),
            start: 0,
            offset: 0,
            len: 100,
            take: Some(0),
            muted: false,
            seq: 0,
            ..Clip::default()
        });
        t.effects.push(Effect {
            id: "e1".into(),
            name: "Drive".into(),
            class: vec![7; 16],
            bundle: String::new(),
            bypass: false,
            state: Some(EffectState {
                component: vec![1, 2, 3],
                controller: None,
            }),
            ..Effect::default()
        });
        p.tracks.push(t);
        p.markers.push(Marker {
            id: "m1".into(),
            bar: 3,
            name: "Riff".into(),
        });
        p.tabs.push(TabBlock {
            id: "b1".into(),
            marker: Some("m1".into()),
            title: "Riff".into(),
            text: "e|--3--|".into(),
            author: String::new(),
            created: 1,
        });
        p.comments.push(Comment {
            id: "x1".into(),
            author: String::new(),
            parent: None,
            track: None,
            start: 0,
            end: None,
            text: "nice".into(),
            resolved: false,
            created: 1,
        });
        p
    }

    #[test]
    fn a_template_keeps_the_set_up_but_not_the_audio_or_comments() {
        let t = set_up().as_template();
        assert!(t.comments.is_empty());
        assert!(t.tracks[0].clips.is_empty());
        assert_eq!(t.tracks[0].gain_db, -6.0);
        assert_eq!(t.tracks[0].effects.len(), 1);
        assert_eq!(t.tempo_bpm, 72.0);
        assert_eq!(t.beats_per_bar, 3);
        assert_eq!(t.notes, "# Warm-up");
        assert_eq!(t.tabs.len(), 1);
    }

    #[test]
    fn a_template_survives_its_file() {
        let t = set_up().as_template();
        assert_eq!(template_from_bytes(&template_to_bytes(&t)).unwrap(), t);
        assert!(template_from_bytes(b"not a template").is_err());
    }

    #[test]
    fn a_project_from_a_template_gets_new_ids_and_keeps_tabs_pinned() {
        let t = set_up().as_template();
        let a = t.from_template(44_100);
        let b = t.from_template(48_000);
        assert_eq!(a.rate, 44_100);
        assert_ne!(a.tracks[0].id, "t1");
        assert_ne!(a.tracks[0].id, b.tracks[0].id);
        assert_ne!(a.tracks[0].effects[0].id, b.tracks[0].effects[0].id);
        assert_ne!(a.markers[0].id, "m1");
        assert_eq!(a.tabs[0].marker.as_ref(), Some(&a.markers[0].id));
        assert_eq!(a.tracks[0].name, "Guitar");
        assert_eq!(
            a.tracks[0].effects[0].state, t.tracks[0].effects[0].state,
            "plugin settings come along"
        );
    }

    #[test]
    fn built_in_templates_start_with_the_empty_one() {
        let all = built_in_templates();
        assert_eq!(all[0].project, Project::default());
        let names: Vec<_> = all.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Empty", "Guitar lesson", "Band demo"]);
        for t in &all {
            let p = t.project.from_template(48_000);
            assert!(p.markers.windows(2).all(|w| w[0].bar < w[1].bar));
            assert!(p.loop_bars.0 < p.loop_bars.1);
        }
    }
}
