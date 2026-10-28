// SPDX-License-Identifier: GPL-3.0-or-later
//! The project document and folder: saving and loading, undo, and
//! merging edits made by two people apart.

use std::path::PathBuf;

use daw_project::{Clip, Effect, EffectState, Marker, Project, ProjectDoc, ProjectFolder, Track};

fn clip(id: &str, start: i64) -> Clip {
    Clip {
        id: id.into(),
        asset: "abc.wav".into(),
        name: "Backing".into(),
        start,
        offset: 0,
        len: 48_000,
        take: None,
        muted: false,
        seq: 0,
    }
}

fn song() -> Project {
    let mut guitar = Track::new("t1".into(), "Guitar".into());
    guitar.clips.push(clip("c1", 0));
    let mut take = clip("c2", 96_000);
    take.take = Some(0);
    take.seq = 1;
    guitar.clips.push(take);
    guitar.effects.push(Effect {
        id: "e1".into(),
        name: "EQ".into(),
        class: vec![7; 16],
        bundle: "C:/Program Files/Common Files/VST3/EQ.vst3".into(),
        bypass: false,
        state: Some(EffectState {
            component: vec![1, 2, 3],
            controller: None,
        }),
    });
    Project {
        tracks: vec![guitar, Track::new("t2".into(), "Bass".into())],
        markers: vec![Marker {
            id: "m1".into(),
            bar: 5,
            name: "Verse".into(),
        }],
        notes: "Watch the bends in bar 7.".into(),
        ..Project::default()
    }
}

#[test]
fn a_project_comes_back_as_it_was_saved() {
    let mut d = ProjectDoc::new(1);
    let p = song();
    assert!(d.set(&p));
    assert!(!d.set(&p), "setting it again changes nothing");
    let back = ProjectDoc::from_bytes(&d.to_bytes(), 2).unwrap();
    assert_eq!(back.get(), p);
}

#[test]
fn undo_and_redo_go_one_edit_at_a_time() {
    let mut d = ProjectDoc::new(1);
    let p0 = song();
    d.set(&p0);
    let mut p1 = p0.clone();
    p1.tracks[0].clips[0].start = 1000;
    d.set(&p1);
    let mut p2 = p1.clone();
    p2.tracks.remove(1);
    p2.tracks[0].name = "Lead".into();
    d.set(&p2);

    assert!(d.undo());
    assert_eq!(d.get(), p1);
    assert!(d.undo());
    assert_eq!(d.get(), p0);
    assert!(d.redo());
    assert_eq!(d.get(), p1);
    assert!(d.redo());
    assert_eq!(d.get(), p2);
}

#[test]
fn plugin_settings_written_on_save_are_not_an_undo_step() {
    let mut d = ProjectDoc::new(1);
    let p0 = song();
    d.set(&p0);
    let mut p1 = p0.clone();
    p1.tracks[0].effects[0].state = Some(EffectState {
        component: vec![9, 9],
        controller: Some(vec![4]),
    });
    d.set_quietly(&p1);
    let mut p2 = p1.clone();
    p2.tempo_bpm = 90.0;
    d.set(&p2);
    assert!(d.undo());
    assert_eq!(
        d.get(),
        p1,
        "the tempo goes back, the plugin's settings stay"
    );
}

#[test]
fn an_effect_without_state_leaves_the_saved_state_alone() {
    let mut d = ProjectDoc::new(1);
    let p = song();
    d.set(&p);
    let mut edit = p.clone();
    edit.tracks[0].effects[0].state = None;
    edit.tracks[0].effects[0].bypass = true;
    d.set(&edit);
    let got = d.get();
    assert!(got.tracks[0].effects[0].bypass);
    assert_eq!(got.tracks[0].effects[0].state, p.tracks[0].effects[0].state);
}

/// Two copies of the same project, edited apart.
fn two_copies() -> (ProjectDoc, ProjectDoc, Project) {
    let mut a = ProjectDoc::new(1);
    let p = song();
    a.set(&p);
    let b = ProjectDoc::from_bytes(&a.to_bytes(), 2).unwrap();
    (a, b, p)
}

fn sync(a: &mut ProjectDoc, b: &mut ProjectDoc) {
    let to_b = a.updates_since(&b.version()).unwrap();
    let to_a = b.updates_since(&a.version()).unwrap();
    b.merge(&to_b).unwrap();
    a.merge(&to_a).unwrap();
}

#[test]
fn edits_to_different_fields_of_one_clip_both_survive() {
    let (mut a, mut b, p) = two_copies();
    let mut pa = p.clone();
    pa.tracks[0].clips[0].start = 5000;
    a.set(&pa);
    let mut pb = p.clone();
    pb.tracks[0].clips[0].len = 1000;
    pb.tracks[0].name = "Rhythm".into();
    b.set(&pb);
    sync(&mut a, &mut b);
    let (ga, gb) = (a.get(), b.get());
    assert_eq!(ga, gb, "both copies agree");
    assert_eq!(ga.tracks[0].clips[0].start, 5000);
    assert_eq!(ga.tracks[0].clips[0].len, 1000);
    assert_eq!(ga.tracks[0].name, "Rhythm");
}

#[test]
fn tracks_and_clips_added_apart_are_all_kept() {
    let (mut a, mut b, p) = two_copies();
    let mut pa = p.clone();
    pa.tracks
        .push(Track::new("ta".into(), "Teacher's part".into()));
    pa.tracks[1].clips.push(clip("ca", 0));
    a.set(&pa);
    let mut pb = p.clone();
    pb.tracks
        .push(Track::new("tb".into(), "Student's part".into()));
    pb.tracks[1].clips.push(clip("cb", 48_000));
    b.set(&pb);
    sync(&mut a, &mut b);
    let g = a.get();
    assert_eq!(g, b.get());
    let names: Vec<&str> = g.tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names.len(), 4);
    assert!(names.contains(&"Teacher's part") && names.contains(&"Student's part"));
    assert_eq!(
        g.tracks[1].clips.len(),
        2,
        "both new clips on the bass track"
    );
}

#[test]
fn notes_typed_apart_are_merged_not_replaced() {
    let (mut a, mut b, p) = two_copies();
    let mut pa = p.clone();
    pa.notes = format!("Warm up first.\n{}", p.notes);
    a.set(&pa);
    let mut pb = p.clone();
    pb.notes = format!("{}\nPractise at 70%.", p.notes);
    b.set(&pb);
    sync(&mut a, &mut b);
    assert_eq!(
        a.get().notes,
        "Warm up first.\nWatch the bends in bar 7.\nPractise at 70%."
    );
}

#[test]
fn undo_takes_back_only_your_own_edits() {
    let (mut a, mut b, p) = two_copies();
    let mut pa = p.clone();
    pa.tempo_bpm = 100.0;
    a.set(&pa);
    let mut pb = p.clone();
    pb.markers[0].name = "Chorus".into();
    b.set(&pb);
    sync(&mut a, &mut b);
    assert!(a.undo());
    let g = a.get();
    assert_eq!(g.tempo_bpm, 120.0, "a's edit is undone");
    assert_eq!(g.markers[0].name, "Chorus", "b's edit stays");
}

#[test]
fn track_order_merges_without_losing_or_doubling_tracks() {
    let (mut a, mut b, p) = two_copies();
    let mut pa = p.clone();
    pa.tracks.swap(0, 1);
    a.set(&pa);
    let mut pb = p.clone();
    pb.tracks.push(Track::new("t3".into(), "Drums".into()));
    b.set(&pb);
    sync(&mut a, &mut b);
    let g = a.get();
    assert_eq!(g, b.get());
    let ids: Vec<&str> = g.tracks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, vec!["t2", "t1", "t3"]);
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("daw-project-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn a_folder_saves_opens_and_keeps_a_backup() {
    let dir = temp("save");
    let f = ProjectFolder::create(&dir).unwrap();
    assert!(
        ProjectFolder::create(&dir).is_ok(),
        "an empty folder is fine"
    );
    let mut d = ProjectDoc::new(1);
    d.set(&song());
    f.save(&d.to_bytes()).unwrap();
    assert!(ProjectFolder::create(&dir).is_err(), "not over a project");
    let mut p = song();
    p.tempo_bpm = 80.0;
    d.set(&p);
    f.save(&d.to_bytes()).unwrap();

    let (g, bytes) = ProjectFolder::open(&dir.join(daw_project::DOC_FILE)).unwrap();
    assert_eq!(g, f);
    assert_eq!(ProjectDoc::from_bytes(&bytes, 3).unwrap().get(), p);
    let old = ProjectDoc::from_bytes(&g.backup().unwrap(), 3).unwrap();
    assert_eq!(old.get().tempo_bpm, 120.0, "the backup is the save before");
}

#[test]
fn audio_is_stored_once_by_content_and_takes_are_moved_in() {
    let dir = temp("assets");
    let f = ProjectFolder::create(&dir).unwrap();
    let src = dir.with_extension("src");
    std::fs::create_dir_all(&src).unwrap();
    let (one, two, other) = (
        src.join("one.wav"),
        src.join("two.WAV"),
        src.join("take.wav"),
    );
    std::fs::write(&one, b"same audio").unwrap();
    std::fs::write(&two, b"same audio").unwrap();
    std::fs::write(&other, b"a take").unwrap();
    let a = f.add_asset(&one, false).unwrap();
    let b = f.add_asset(&two, false).unwrap();
    assert_eq!(a, b);
    assert!(a.ends_with(".wav") && a.len() == 32 + 4);
    assert_eq!(
        std::fs::read_dir(dir.join(daw_project::ASSETS))
            .unwrap()
            .count(),
        1
    );
    assert!(one.exists(), "imports are copied");
    let t = f.add_asset(&other, true).unwrap();
    assert!(!other.exists(), "takes are moved");
    assert_eq!(std::fs::read(f.asset(&t)).unwrap(), b"a take");
    assert!(f.has_asset(&t));
    assert!(
        !f.has_asset(&other.to_string_lossy()),
        "a path elsewhere is not an asset"
    );
    assert!(!f.has_asset("missing.wav"));
}
