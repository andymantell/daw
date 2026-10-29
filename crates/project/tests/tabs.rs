// SPDX-License-Identifier: GPL-3.0-or-later
//! Lesson notes' tab blocks in the document (roadmap 1.7).

use daw_project::{Marker, Project, ProjectDoc, TabBlock};

fn block(id: &str, marker: Option<&str>, text: &str) -> TabBlock {
    TabBlock {
        id: id.into(),
        marker: marker.map(Into::into),
        title: format!("Riff {id}"),
        text: text.into(),
        author: "ab12".into(),
        created: 1_700_000_000,
    }
}

fn song() -> Project {
    Project {
        markers: vec![Marker {
            id: "verse".into(),
            bar: 5,
            name: "Verse".into(),
        }],
        notes: "# Week 3\n- **Slow** the bridge down\n".into(),
        ..Project::default()
    }
}

#[test]
fn tab_blocks_come_back_as_they_were_saved() {
    let mut d = ProjectDoc::new(1);
    let mut p = song();
    p.tabs = vec![
        block("t1", Some("verse"), "e|-----0-3-|\nB|---1-----|"),
        block("t2", None, "E|-0-0-3-|"),
    ];
    p.tabs[1].created += 1;
    assert!(d.set(&p));
    assert!(!d.set(&p), "setting it again changes nothing");
    let back = ProjectDoc::from_bytes(&d.to_bytes(), 2).unwrap();
    assert_eq!(back.get(), p);
}

#[test]
fn older_projects_have_no_tab_blocks() {
    let mut d = ProjectDoc::new(1);
    d.set(&song());
    assert!(
        ProjectDoc::from_bytes(&d.to_bytes(), 2)
            .unwrap()
            .get()
            .tabs
            .is_empty()
    );
}

#[test]
fn blocks_added_and_changed_apart_are_all_kept() {
    let mut base = ProjectDoc::new(1);
    let mut p = song();
    p.tabs = vec![block("t1", Some("verse"), "e|---|")];
    base.set(&p);
    let mut a = ProjectDoc::from_bytes(&base.to_bytes(), 2).unwrap();
    let mut b = ProjectDoc::from_bytes(&base.to_bytes(), 3).unwrap();

    // The teacher adds a block; the student moves the first to the whole
    // song and retitles it.
    let mut pa = a.get();
    pa.tabs.push(block("t2", Some("verse"), "A|-2-4-|"));
    pa.tabs[1].created += 5;
    a.set(&pa);
    let mut pb = b.get();
    pb.tabs[0].marker = None;
    pb.tabs[0].title = "Warm-up".into();
    b.set(&pb);

    a.merge(&b.to_bytes()).unwrap();
    b.merge(&a.to_bytes()).unwrap();
    assert_eq!(a.get(), b.get());
    let tabs = a.get().tabs;
    assert_eq!(tabs.len(), 2);
    assert_eq!(tabs[0].marker, None);
    assert_eq!(tabs[0].title, "Warm-up");
    assert_eq!(tabs[0].text, "e|---|");
    assert_eq!(tabs[1].text, "A|-2-4-|");
}

#[test]
fn deleting_a_marker_leaves_its_blocks() {
    let mut d = ProjectDoc::new(1);
    let mut p = song();
    p.tabs = vec![block("t1", Some("verse"), "e|---|")];
    d.set(&p);
    p.markers.clear();
    d.set(&p);
    let back = d.get();
    assert_eq!(back.tabs.len(), 1, "the block stays");
    assert_eq!(back.tabs[0].marker.as_deref(), Some("verse"));
}

#[test]
fn deleting_a_block_can_be_undone() {
    let mut d = ProjectDoc::new(1);
    let mut p = song();
    p.tabs = vec![block("t1", Some("verse"), "e|---|")];
    d.set(&p);
    p.tabs.clear();
    d.set(&p);
    assert!(d.get().tabs.is_empty());
    assert!(d.undo());
    assert_eq!(d.get().tabs[0].text, "e|---|");
}
