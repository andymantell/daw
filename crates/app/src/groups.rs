// SPDX-License-Identifier: GPL-3.0-or-later
//! Edit groups (roadmap 2.1): tracks edited together, such as the mics
//! on a drum kit. Selecting a clip on one track of a group selects the
//! clips at the same time on the others, so splitting, moving, nudging,
//! deleting, crossfading and fading act on them all, and trimming or
//! fading one clip's edge by dragging moves the same edge on the others
//! by the same amount. The tracks stay in time with each other.
//!
//! A group can be switched off, keeping its tracks, and group editing as
//! a whole can be switched off from the timeline's toolbar (or with G)
//! for a one-off edit to a single track. A clip's volume is never
//! grouped, as each mic wants its own level.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use daw_project::EditGroup;
use eframe::egui::{self, RichText};

use crate::App;
use crate::timeline::{Clip, Song};

/// An active group as the timeline sees it: its tracks by position.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaneGroup {
    pub name: String,
    pub lanes: Vec<usize>,
}

/// The group track `lane` is in, if any.
pub fn group_of(groups: &[LaneGroup], lane: usize) -> Option<&LaneGroup> {
    groups.iter().find(|g| g.lanes.contains(&lane))
}

/// Whether two clips on grouped tracks are edited together: they
/// overlap in time and, when both are recorded takes, are the same take.
fn goes_with(a: &Clip, b: &Clip) -> bool {
    let takes = match (a.take, b.take) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    };
    takes && a.start < b.end() && b.start < a.end()
}

/// The clips edited with clip `id`: on the other tracks of its group,
/// at the same time. Empty if its track is in no active group.
pub fn partners(song: &Song, groups: &[LaneGroup], id: u64) -> Vec<u64> {
    let Some(lane) = song.lane_of(id) else {
        return Vec::new();
    };
    let Some(g) = group_of(groups, lane) else {
        return Vec::new();
    };
    let Some(c) = song.lanes[lane].clips.iter().find(|c| c.id == id) else {
        return Vec::new();
    };
    (g.lanes.iter())
        .filter(|&&l| l != lane)
        .filter_map(|&l| song.lanes.get(l))
        .flat_map(|l| l.clips.iter())
        .filter(|o| goes_with(c, o))
        .map(|o| o.id)
        .collect()
}

/// `ids` and every clip edited with them.
pub fn with_partners(
    song: &Song,
    groups: &[LaneGroup],
    ids: impl IntoIterator<Item = u64>,
) -> HashSet<u64> {
    let mut out = HashSet::new();
    for id in ids {
        out.extend(partners(song, groups, id));
        out.insert(id);
    }
    out
}

/// The active groups in `groups` by track position, given the tracks'
/// ids in order. Groups with fewer than two tracks change nothing and
/// are left out.
pub fn lane_groups(groups: &[EditGroup], tracks: &[&str]) -> Vec<LaneGroup> {
    (groups.iter())
        .filter(|g| g.active)
        .map(|g| LaneGroup {
            name: g.name.clone(),
            lanes: (tracks.iter().enumerate())
                .filter(|(_, t)| g.tracks.iter().any(|k| k == *t))
                .map(|(i, _)| i)
                .collect(),
        })
        .filter(|g| g.lanes.len() > 1)
        .collect()
}

/// Puts track `track` in group `to` (or in none), taking it out of any
/// other: a track is in one group at most.
pub fn assign(groups: &mut [EditGroup], track: &str, to: Option<&str>) {
    for g in groups.iter_mut() {
        let want = Some(g.id.as_str()) == to;
        let has = g.tracks.iter().any(|t| t == track);
        if want && !has {
            g.tracks.push(track.to_string());
            g.tracks.sort();
        } else if !want && has {
            g.tracks.retain(|t| t != track);
        }
    }
}

/// A new group named `name` of `tracks`, which leave any group they
/// were in.
pub fn add_group(groups: &mut Vec<EditGroup>, name: String, tracks: &[String]) -> String {
    let id = daw_project::new_id();
    groups.push(EditGroup {
        id: id.clone(),
        name,
        tracks: Vec::new(),
        active: true,
        created: now(),
    });
    for t in tracks {
        assign(groups, t, Some(&id));
    }
    id
}

/// A name for a new group not already used: "Group 1", "Group 2" and so
/// on.
fn new_name(groups: &[EditGroup]) -> String {
    (1..)
        .map(|n| format!("Group {n}"))
        .find(|n| !groups.iter().any(|g| &g.name == n))
        .expect("an unused name")
}

/// The Edit groups window.
#[derive(Default)]
pub(crate) struct Groups {
    pub open: bool,
}

impl App {
    /// The project's groups as the timeline needs them this frame.
    pub(crate) fn lane_groups(&self) -> Vec<LaneGroup> {
        let ids: Vec<&str> = self.tracks.iter().map(|t| t.key.as_str()).collect();
        lane_groups(&self.edit_groups, &ids)
    }

    /// The groups as they go into the project: tracks since removed are
    /// left out.
    pub(crate) fn edit_groups_value(&self) -> Vec<EditGroup> {
        let mut out = self.edit_groups.clone();
        for g in &mut out {
            g.tracks.retain(|k| self.tracks.iter().any(|t| &t.key == k));
        }
        out
    }

    pub(crate) fn groups_window(&mut self, ctx: &egui::Context) {
        if !self.groups_ui.open {
            return;
        }
        let may_edit = self.may_edit_song();
        let mut open = true;
        let names: Vec<(String, String)> = (self.tracks.iter())
            .map(|t| (t.key.clone(), t.name.clone()))
            .collect();
        // The tracks of the selected clips, for a new group.
        let chosen: Vec<String> = (self.tracks.iter())
            .filter(|t| {
                t.clips
                    .iter()
                    .any(|c| self.timeline.selected.contains(&c.id))
            })
            .map(|t| t.key.clone())
            .collect();
        let groups = &mut self.edit_groups;
        egui::Window::new("Edit groups")
            .open(&mut open)
            .resizable(true)
            .default_width(340.0)
            .show(ctx, |ui| {
                ui.label(
                    "Clips on grouped tracks are selected, moved, split, trimmed and \
                     faded together. Volume stays separate.",
                );
                if !may_edit {
                    ui.label(
                        RichText::new("You can comment on this project, not change it.").weak(),
                    );
                }
                ui.add_enabled_ui(may_edit, |ui| {
                    let mut remove = None;
                    let mut moves: Vec<(String, Option<String>)> = Vec::new();
                    for (gi, g) in groups.iter_mut().enumerate() {
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut g.name)
                                    .desired_width(150.0)
                                    .char_limit(40),
                            )
                            .accessible_name("Group name");
                            ui.checkbox(&mut g.active, "On")
                                .on_hover_text("Off, its tracks are edited one at a time");
                            if ui.button("Remove").clicked() {
                                remove = Some(gi);
                            }
                        });
                        ui.horizontal_wrapped(|ui| {
                            for (key, name) in &names {
                                let mut on = g.tracks.contains(key);
                                if ui.checkbox(&mut on, name).changed() {
                                    moves.push((key.clone(), on.then(|| g.id.clone())));
                                }
                            }
                        });
                    }
                    for (track, to) in moves {
                        assign(groups, &track, to.as_deref());
                    }
                    if let Some(i) = remove {
                        groups.remove(i);
                    }
                    ui.separator();
                    let text = if chosen.len() > 1 {
                        "New group of the selected clips' tracks"
                    } else {
                        "New group"
                    };
                    if ui.button(text).clicked() {
                        let name = new_name(groups);
                        add_group(groups, name, if chosen.len() > 1 { &chosen } else { &[] });
                    }
                });
            });
        self.groups_ui.open = open;
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::Lane;
    use crate::timeline::tests::clip;

    fn group(name: &str, lanes: &[usize]) -> LaneGroup {
        LaneGroup {
            name: name.into(),
            lanes: lanes.to_vec(),
        }
    }

    #[test]
    fn partners_are_the_clips_at_the_same_time_on_the_other_tracks() {
        let mut kick = vec![clip(0, 100), clip(500, 100)];
        let mut snare = vec![clip(50, 100), clip(300, 100)];
        let mut bass = vec![clip(0, 100)];
        let ids = (kick[0].id, kick[1].id, snare[0].id, snare[1].id, bass[0].id);
        let song = Song {
            lanes: vec![
                Lane {
                    name: "Kick",
                    clips: &mut kick,
                    kind: daw_project::TrackKind::Audio,
                    in_folder: false,
                    notes: Box::leak(Box::default()),
                },
                Lane {
                    name: "Snare",
                    clips: &mut snare,
                    kind: daw_project::TrackKind::Audio,
                    in_folder: false,
                    notes: Box::leak(Box::default()),
                },
                Lane {
                    name: "Bass",
                    clips: &mut bass,
                    kind: daw_project::TrackKind::Audio,
                    in_folder: false,
                    notes: Box::leak(Box::default()),
                },
            ],
            markers: &mut Vec::new(),
        };
        let groups = [group("Drums", &[0, 1])];
        assert_eq!(partners(&song, &groups, ids.0), [ids.2]);
        assert_eq!(partners(&song, &groups, ids.2), [ids.0]);
        // Nothing at that time on the snare.
        assert!(partners(&song, &groups, ids.1).is_empty());
        // Not grouped.
        assert!(partners(&song, &groups, ids.4).is_empty());
        assert!(partners(&song, &[], ids.0).is_empty());
        let all = with_partners(&song, &groups, [ids.0, ids.3]);
        assert_eq!(all, HashSet::from([ids.0, ids.2, ids.3]));
    }

    #[test]
    fn takes_go_with_the_same_take_on_the_other_tracks() {
        let take = |n: usize| {
            let mut c = clip(0, 100);
            c.take = Some(n);
            c
        };
        let mut a = vec![take(0), take(1)];
        let mut b = vec![take(0), take(1)];
        let (a1, b1) = (a[1].id, b[1].id);
        let song = Song {
            lanes: vec![
                Lane {
                    name: "Overhead",
                    clips: &mut a,
                    kind: daw_project::TrackKind::Audio,
                    in_folder: false,
                    notes: Box::leak(Box::default()),
                },
                Lane {
                    name: "Room",
                    clips: &mut b,
                    kind: daw_project::TrackKind::Audio,
                    in_folder: false,
                    notes: Box::leak(Box::default()),
                },
            ],
            markers: &mut Vec::new(),
        };
        assert_eq!(partners(&song, &[group("Kit", &[0, 1])], a1), [b1]);
    }

    #[test]
    fn a_track_is_in_one_group_at_most() {
        let mut gs = Vec::new();
        let drums = add_group(&mut gs, "Drums".into(), &["kick".into(), "snare".into()]);
        let gtr = add_group(&mut gs, "Guitars".into(), &["snare".into(), "gtr".into()]);
        assert_eq!(gs[0].tracks, ["kick"]);
        assert_eq!(gs[1].tracks, ["gtr", "snare"]);
        assign(&mut gs, "gtr", Some(&drums));
        assert_eq!(gs[0].tracks, ["gtr", "kick"]);
        assert_eq!(gs[1].tracks, ["snare"]);
        assign(&mut gs, "snare", None);
        assert!(gs[1].tracks.is_empty());
        assert_ne!(drums, gtr);
        assert_eq!(new_name(&gs), "Group 1");
    }

    #[test]
    fn only_active_groups_of_two_or_more_tracks_reach_the_timeline() {
        let mut gs = Vec::new();
        add_group(&mut gs, "Drums".into(), &["kick".into(), "snare".into()]);
        add_group(&mut gs, "Solo".into(), &["gtr".into()]);
        let tracks = ["gtr", "snare", "bass", "kick"];
        assert_eq!(lane_groups(&gs, &tracks), [group("Drums", &[1, 3])]);
        gs[0].active = false;
        assert!(lane_groups(&gs, &tracks).is_empty());
    }
}
