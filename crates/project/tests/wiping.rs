// SPDX-License-Identifier: GPL-3.0-or-later
//! Deleted lesson words and takes really leave the project (the owner's
//! choice, 2026-11-05): tab blocks deleted or rewritten, letters deleted
//! from the lesson notes, and audio nothing needs any more.

use daw_project::{Clip, Project, ProjectDoc, ProjectFolder, TabBlock, Track};
use loro::{LoroDoc, VersionVector};

const SECRET: &str = "Sam's phone is 07700 900123";

/// Everything a saved file holds, history included, as text.
fn everything_in(bytes: &[u8]) -> String {
    let doc = LoroDoc::new();
    doc.import(bytes).unwrap();
    let json = doc
        .export_json_updates(&VersionVector::new(), &doc.oplog_vv())
        .unwrap();
    format!("{json:?} {:?}", doc.get_deep_value())
}

fn contains(bytes: &[u8], text: &str) -> bool {
    bytes.windows(text.len()).any(|w| w == text.as_bytes()) || everything_in(bytes).contains(text)
}

fn tab(id: &str, title: &str, text: &str) -> TabBlock {
    TabBlock {
        id: id.into(),
        marker: None,
        title: title.into(),
        text: text.into(),
        author: String::new(),
        created: 1,
    }
}

fn with_tabs(tabs: Vec<TabBlock>) -> Project {
    Project {
        tabs,
        ..Project::default()
    }
}

fn with_notes(notes: &str) -> Project {
    Project {
        notes: notes.into(),
        ..Project::default()
    }
}

#[test]
fn a_deleted_tab_block_is_gone_from_the_saved_file() {
    let mut d = ProjectDoc::new(1);
    d.set(&with_tabs(vec![
        tab("t1", "Riff", &format!("e|--3--| {SECRET}")),
        tab("t2", "Solo", "e|--5--|"),
    ]));
    assert!(contains(&d.raw_bytes(), SECRET), "the history holds it");
    d.set(&with_tabs(vec![tab("t2", "Solo", "e|--5--|")]));
    assert!(d.take_dropped(), "the backup must go too");
    let bytes = d.to_bytes();
    assert!(!contains(&bytes, SECRET));
    assert!(contains(&bytes, "e|--5--|"));
    assert_eq!(ProjectDoc::from_bytes(&bytes, 2).unwrap().get(), d.get());
}

#[test]
fn a_rewritten_tab_block_keeps_only_its_new_words() {
    let mut d = ProjectDoc::new(1);
    d.set(&with_tabs(vec![tab("t1", SECRET, "e|--3--|")]));
    d.take_dropped();
    d.set(&with_tabs(vec![tab("t1", "Riff", "e|--3--|")]));
    assert!(d.take_dropped());
    let bytes = d.to_bytes();
    assert!(!contains(&bytes, SECRET));
    assert_eq!(ProjectDoc::from_bytes(&bytes, 2).unwrap().get(), d.get());
}

#[test]
fn words_deleted_from_the_notes_are_gone_from_the_saved_file() {
    let mut d = ProjectDoc::new(1);
    d.set(&with_notes(&format!(
        "Bends in bar 7. {SECRET}. Practise slowly."
    )));
    d.take_dropped();
    d.set(&with_notes("Bends in bar 7. Practise slowly."));
    assert!(contains(&d.raw_bytes(), "07700"), "the history holds it");
    assert!(d.take_dropped());
    let bytes = d.to_bytes();
    assert!(!contains(&bytes, "07700"));
    assert!(!contains(&bytes, "phone"));
    let back = ProjectDoc::from_bytes(&bytes, 2).unwrap();
    assert_eq!(back.get().notes, "Bends in bar 7. Practise slowly.");
}

#[test]
fn rewritten_notes_in_any_script_come_back_as_they_are_now() {
    // Letters of every size are blanked with ones the same size, so the
    // history still lines up.
    let mut d = ProjectDoc::new(1);
    d.set(&with_notes("Bar 7 ♯ é 🎸 secret words 𝄞 here"));
    d.set(&with_notes("Bar 7 here"));
    d.set(&with_notes("Bar 7 ♭ 🎵 here, again"));
    let bytes = d.to_bytes();
    assert!(!contains(&bytes, "secret"));
    assert!(!contains(&bytes, "🎸"));
    let back = ProjectDoc::from_bytes(&bytes, 2).unwrap();
    assert_eq!(back.get().notes, "Bar 7 ♭ 🎵 here, again");
    // And it still takes edits and merges.
    let mut b = ProjectDoc::from_bytes(&d.raw_bytes(), 3).unwrap();
    b.set(&with_notes("Bar 7 ♭ 🎵 here, again. Slower."));
    let mut a = back;
    a.merge(&b.to_bytes()).unwrap();
    assert_eq!(a.get().notes, "Bar 7 ♭ 🎵 here, again. Slower.");
    assert!(!contains(&a.to_bytes(), "secret"));
}

#[test]
fn adding_to_the_notes_keeps_the_backup() {
    let mut d = ProjectDoc::new(1);
    d.set(&with_notes("Bar 7"));
    d.take_dropped();
    d.set(&with_notes("Warm up. Bar 7, slowly"));
    assert!(!d.take_dropped(), "nothing was taken out");
}

#[test]
fn a_copy_that_still_has_the_words_does_not_bring_them_back() {
    let mut a = ProjectDoc::new(1);
    a.set(&with_notes(&format!("Bar 7. {SECRET}")));
    let mut b = ProjectDoc::from_bytes(&a.raw_bytes(), 2).unwrap();
    a.set(&with_notes("Bar 7."));
    b.set(&Project {
        notes: format!("Bar 7. {SECRET}"),
        tabs: vec![tab("t1", "Riff", "e|--3--|")],
        ..Project::default()
    });
    let mut a2 = ProjectDoc::from_bytes(&a.to_bytes(), 3).unwrap();
    a2.merge(&b.raw_bytes()).unwrap();
    assert_eq!(a2.get().notes, "Bar 7.");
    assert_eq!(a2.get().tabs.len(), 1);
    assert!(!contains(&a2.to_bytes(), "07700"));
}

#[test]
fn the_backup_does_not_keep_deleted_notes() {
    let dir = std::env::temp_dir().join(format!("daw-wipe-{}", daw_project::new_id()));
    let f = ProjectFolder::create(&dir).unwrap();
    let mut d = ProjectDoc::new(1);
    d.set(&with_notes(SECRET));
    f.save(&d.to_bytes()).unwrap();
    d.take_dropped();
    d.set(&with_notes("Bar 7"));
    assert!(d.take_dropped());
    f.save_dropping_backup(&d.to_bytes()).unwrap();
    assert!(!contains(&f.backup().unwrap(), "07700"));
    std::fs::remove_dir_all(&dir).unwrap();
}

// Audio nothing needs any more.

fn clip(id: &str, asset: &str) -> Clip {
    Clip {
        id: id.into(),
        asset: asset.into(),
        name: "Take".into(),
        len: 48_000,
        ..Default::default()
    }
}

fn with_clips(clips: Vec<Clip>) -> Project {
    let mut t = Track::new("t1".into(), "Guitar".into());
    t.clips = clips;
    Project {
        tracks: vec![t],
        ..Project::default()
    }
}

/// A project folder holding two takes, with both on the timeline.
fn folder_with_two_takes() -> (ProjectFolder, String, String) {
    let dir = std::env::temp_dir().join(format!("daw-unused-{}", daw_project::new_id()));
    let f = ProjectFolder::create(&dir).unwrap();
    let mut names = Vec::new();
    for (i, body) in [b"RIFF take one".as_slice(), b"RIFF take two"]
        .iter()
        .enumerate()
    {
        let src = dir.join(format!("take{i}.wav"));
        std::fs::write(&src, body).unwrap();
        names.push(f.add_asset(&src, true).unwrap());
    }
    (f, names[0].clone(), names[1].clone())
}

fn names(v: Vec<daw_project::AudioFile>) -> Vec<String> {
    v.into_iter().map(|f| f.name).collect()
}

#[test]
fn a_deleted_take_goes_once_undo_cant_bring_it_back() {
    let (f, one, two) = folder_with_two_takes();
    let mut d = ProjectDoc::new(1);
    d.set(&with_clips(vec![clip("c1", &one), clip("c2", &two)]));
    d.set(&with_clips(vec![clip("c2", &two)]));
    f.save(&d.to_bytes()).unwrap();
    // Undo could still bring it back here.
    assert!(f.unused_audio(&d, "me", true).is_empty());
    // Opened again: nothing to undo.
    let d = ProjectDoc::from_bytes(&d.to_bytes(), 2).unwrap();
    let gone = f.remove_unused_audio(&d, "me", true);
    assert_eq!(names(gone), std::slice::from_ref(&one));
    assert!(!f.has_asset(&one));
    assert!(f.has_asset(&two));
    assert!(f.unused_audio(&d, "me", true).is_empty(), "once");
    std::fs::remove_dir_all(&f.dir).unwrap();
}

#[test]
fn audio_deleted_after_opening_waits_for_undo() {
    let (f, one, two) = folder_with_two_takes();
    let mut d = ProjectDoc::new(1);
    d.set(&with_clips(vec![clip("c1", &one), clip("c2", &two)]));
    let mut d = ProjectDoc::from_bytes(&d.to_bytes(), 2).unwrap();
    d.set(&with_clips(vec![clip("c2", &two)]));
    assert!(
        f.unused_audio(&d, "me", true).is_empty(),
        "undo would bring it back"
    );
    // Closing the project ends undo.
    assert_eq!(
        names(f.unused_audio(&d, "me", false)),
        std::slice::from_ref(&one)
    );
    assert!(d.undo());
    assert_eq!(d.get().tracks[0].clips.len(), 2);
    std::fs::remove_dir_all(&f.dir).unwrap();
}

#[test]
fn a_snapshot_keeps_the_audio_it_would_bring_back() {
    let (f, one, two) = folder_with_two_takes();
    let mut d = ProjectDoc::new(1);
    d.set(&with_clips(vec![clip("c1", &one), clip("c2", &two)]));
    let snap = d.take_snapshot("Lesson 3", "", 10);
    d.set(&with_clips(vec![clip("c2", &two)]));
    let mut d = ProjectDoc::from_bytes(&d.to_bytes(), 2).unwrap();
    assert!(f.unused_audio(&d, "me", true).is_empty());
    d.delete_snapshot(&snap);
    let d = ProjectDoc::from_bytes(&d.to_bytes(), 3).unwrap();
    assert_eq!(names(f.unused_audio(&d, "me", true)), [one]);
    std::fs::remove_dir_all(&f.dir).unwrap();
}

#[test]
fn audio_this_copy_has_never_heard_of_is_left_alone() {
    // Another computer's take can arrive in a synced folder before its
    // document does.
    let (f, one, two) = folder_with_two_takes();
    let d = ProjectDoc::new(1);
    let d = ProjectDoc::from_bytes(&d.to_bytes(), 2).unwrap();
    assert!(f.unused_audio(&d, "me", true).is_empty());
    assert!(f.has_asset(&one) && f.has_asset(&two));
    std::fs::remove_dir_all(&f.dir).unwrap();
}

#[test]
fn audio_another_computer_still_uses_is_kept() {
    let (f, one, two) = folder_with_two_takes();
    let mut d = ProjectDoc::new(1);
    d.set(&with_clips(vec![clip("c1", &one), clip("c2", &two)]));
    // The other computer, not yet told of the deletion, saves its copy.
    let theirs = ProjectDoc::from_bytes(&d.to_bytes(), 5).unwrap();
    f.save_device("them", &theirs.to_bytes()).unwrap();
    d.set(&with_clips(vec![clip("c2", &two)]));
    let d = ProjectDoc::from_bytes(&d.to_bytes(), 2).unwrap();
    f.save_device("me", &d.to_bytes()).unwrap();
    assert!(f.unused_audio(&d, "me", true).is_empty());
    // Once they have it too, it goes.
    let mut theirs = theirs;
    theirs.merge(&d.to_bytes()).unwrap();
    f.save_device("them", &theirs.to_bytes()).unwrap();
    assert_eq!(names(f.unused_audio(&d, "me", true)), [one]);
    std::fs::remove_dir_all(&f.dir).unwrap();
}

#[test]
fn a_snapshot_from_before_a_deletion_restores_without_the_words() {
    let mut d = ProjectDoc::new(1);
    d.set(&Project {
        notes: format!("é🎸 Bar 7. {SECRET}. Slowly."),
        tabs: vec![tab("t1", "Riff", SECRET), tab("t2", "Solo", "e|--5--|")],
        ..Project::default()
    });
    let snap = d.take_snapshot("Lesson 3", "", 10);
    d.set(&Project {
        notes: "é🎸 Bar 7. Slowly. Then faster.".into(),
        tabs: vec![tab("t2", "Solo", "e|--7--|")],
        ..Project::default()
    });
    let reopened = ProjectDoc::from_bytes(&d.to_bytes(), 2).unwrap();
    for mut d in [d, reopened] {
        let p = d.restore_snapshot(&snap).unwrap();
        assert_eq!(p.notes, "é🎸 Bar 7. Slowly.");
        assert_eq!(p.tabs, [tab("t2", "Solo", "e|--7--|")]);
    }
}
