// SPDX-License-Identifier: GPL-3.0-or-later
//! Pinned comments in the document, and taking deleted comments out of
//! the saved history.

use daw_project::{Comment, Project, ProjectDoc};
use loro::{LoroDoc, VersionVector};

const SECRET: &str = "Sam's phone is 07700 900123";

fn comment(id: &str, text: &str) -> Comment {
    Comment {
        id: id.into(),
        author: "ab12".into(),
        parent: None,
        track: Some("t1".into()),
        start: 96_000,
        end: Some(192_000),
        text: text.into(),
        resolved: false,
        created: 1_700_000_000,
    }
}

fn with(comments: Vec<Comment>) -> Project {
    Project {
        comments,
        ..Project::default()
    }
}

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

#[test]
fn comments_come_back_as_they_were_saved() {
    let mut d = ProjectDoc::new(1);
    let mut reply = comment("c2", "Better now");
    reply.parent = Some("c1".into());
    reply.track = None;
    reply.end = None;
    reply.created += 60;
    let p = with(vec![comment("c1", "Rushing here"), reply]);
    assert!(d.set(&p));
    assert!(!d.set(&p), "setting it again changes nothing");
    let back = ProjectDoc::from_bytes(&d.to_bytes(), 2).unwrap();
    assert_eq!(back.get(), p);
}

#[test]
fn the_history_would_keep_deleted_text_without_scrubbing() {
    // Shows the test below is checking something real.
    let mut d = ProjectDoc::new(1);
    d.set(&with(vec![comment("c1", SECRET)]));
    d.set(&with(vec![]));
    assert!(contains(&d.raw_bytes(), SECRET));
}

#[test]
fn a_deleted_comment_is_gone_from_the_saved_file() {
    let mut d = ProjectDoc::new(1);
    d.set(&with(vec![comment("c1", SECRET), comment("c2", "Keep me")]));
    d.set(&with(vec![comment("c2", "Keep me")]));
    let bytes = d.to_bytes();
    assert!(!contains(&bytes, SECRET));
    assert!(contains(&bytes, "Keep me"));
    let back = ProjectDoc::from_bytes(&bytes, 2).unwrap();
    assert_eq!(back.get(), d.get());
}

#[test]
fn old_text_of_a_changed_comment_is_gone_too() {
    let mut d = ProjectDoc::new(1);
    d.set(&with(vec![comment("c1", SECRET)]));
    d.set(&with(vec![comment("c1", "Call me")]));
    let bytes = d.to_bytes();
    assert!(!contains(&bytes, SECRET));
    assert_eq!(ProjectDoc::from_bytes(&bytes, 2).unwrap().get(), d.get());
}

#[test]
fn a_file_saved_before_scrubbing_is_cleaned_on_the_next_save() {
    let mut d = ProjectDoc::new(1);
    d.set(&with(vec![comment("c1", SECRET)]));
    d.set(&with(vec![]));
    let old = ProjectDoc::from_bytes(&d.raw_bytes(), 2).unwrap();
    assert!(!contains(&old.to_bytes(), SECRET));
}

#[test]
fn a_scrubbed_copy_still_merges_with_one_that_was_not() {
    // Two people share a project with a comment; one deletes it while
    // the other, not yet told, adds a reply and another comment.
    let mut a = ProjectDoc::new(1);
    a.set(&with(vec![comment("c1", SECRET)]));
    let mut b = ProjectDoc::from_bytes(&a.raw_bytes(), 2).unwrap();
    a.set(&with(vec![]));
    let mut reply = comment("c2", "Got it");
    reply.parent = Some("c1".into());
    b.set(&with(vec![
        comment("c1", SECRET),
        reply,
        comment("c3", "Bar 9"),
    ]));

    // a's scrubbed file reaches b, and b's unscrubbed one reaches a.
    let mut a2 = ProjectDoc::from_bytes(&a.to_bytes(), 3).unwrap();
    a2.merge(&b.raw_bytes()).unwrap();
    b.merge(&a.to_bytes()).unwrap();
    assert_eq!(a2.get(), b.get());
    let ids: Vec<String> = b.get().comments.into_iter().map(|c| c.id).collect();
    assert_eq!(ids, ["c2", "c3"], "the reply stays, its parent is gone");

    // Merging b's old history in does not bring the text back.
    assert!(!contains(&a2.to_bytes(), SECRET));
    assert!(!contains(&b.to_bytes(), SECRET));
}

#[test]
fn undoing_a_delete_brings_the_comment_back_and_saves_it() {
    let mut d = ProjectDoc::new(1);
    d.set(&with(vec![comment("c1", "Rushing here")]));
    d.set(&with(vec![]));
    let _ = d.to_bytes();
    assert!(d.undo());
    assert_eq!(d.get().comments.len(), 1);
    let back = ProjectDoc::from_bytes(&d.to_bytes(), 2).unwrap();
    assert_eq!(back.get().comments[0].text, "Rushing here");
}

#[test]
fn other_parts_of_the_project_keep_their_history() {
    let mut d = ProjectDoc::new(1);
    let mut p = with(vec![comment("c1", SECRET)]);
    p.notes = "First notes".into();
    d.set(&p);
    p.notes = "Second notes".into();
    p.comments.clear();
    d.set(&p);
    let bytes = d.to_bytes();
    assert!(!contains(&bytes, SECRET));
    assert!(
        everything_in(&bytes).contains("First notes"),
        "only comments are scrubbed for now"
    );
}

#[test]
fn the_backup_does_not_keep_a_deleted_comment() {
    let dir = std::env::temp_dir().join(format!("daw-comments-{}", daw_project::new_id()));
    let f = daw_project::ProjectFolder::create(&dir).unwrap();
    let mut d = ProjectDoc::new(1);
    d.set(&with(vec![comment("c1", SECRET)]));
    f.save(&d.to_bytes()).unwrap();
    assert!(!d.take_dropped());
    d.set(&with(vec![]));
    assert!(d.take_dropped());
    f.save_dropping_backup(&d.to_bytes()).unwrap();
    assert!(!d.take_dropped(), "asked once");
    let backup = f.backup().unwrap();
    assert!(!contains(&backup, SECRET));
    let (_, saved) = daw_project::ProjectFolder::open(&dir).unwrap();
    assert!(!contains(&saved, SECRET));
    std::fs::remove_dir_all(&dir).unwrap();
}
