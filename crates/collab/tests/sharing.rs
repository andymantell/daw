// SPDX-License-Identifier: GPL-3.0-or-later
//! Sharing a project without a server: bundles, and copies of the
//! document written into a shared folder by each computer.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use daw_collab::bundle;
use daw_collab::watch::{Change, FolderWatch};
use daw_project::{Clip, Project, ProjectDoc, ProjectFolder, Track};

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("daw-collab-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A saved project with one clip of real audio, as its creator has it.
fn project(dir: &Path) -> (ProjectFolder, ProjectDoc, Project) {
    let f = ProjectFolder::create(&dir.join("Song")).unwrap();
    let wav = dir.join("backing.wav");
    std::fs::write(&wav, b"backing audio").unwrap();
    let asset = f.add_asset(&wav, false).unwrap();
    let mut t = Track::new("t1".into(), "Backing".into());
    t.clips.push(Clip {
        id: "c1".into(),
        asset,
        name: "Backing".into(),
        start: 0,
        offset: 0,
        len: 48_000,
        take: None,
        muted: false,
        seq: 0,
    });
    let p = Project {
        tracks: vec![t],
        ..Project::default()
    };
    let mut d = ProjectDoc::new(1);
    d.set(&p);
    f.save(&d.to_bytes()).unwrap();
    (f, d, p)
}

fn assets(p: &Project) -> Vec<String> {
    p.tracks
        .iter()
        .flat_map(|t| &t.clips)
        .map(|c| c.asset.clone())
        .collect()
}

#[test]
fn a_bundle_carries_the_document_and_its_audio() {
    let dir = temp("bundle");
    let (f, d, p) = project(&dir);
    std::fs::create_dir_all(f.dir.join("Exports")).unwrap();
    std::fs::write(f.dir.join("Exports").join("mix.wav"), b"left out").unwrap();
    let file = dir.join("Song.dawbundle");
    bundle::write(&file, "Song", &d.to_bytes(), &f, &assets(&p)).unwrap();
    assert!(bundle::is_bundle(&file));

    let peek = bundle::peek(&file).unwrap();
    assert_eq!(peek.name, "Song");
    let other = ProjectFolder::create(&dir.join("Elsewhere")).unwrap();
    let b = bundle::unpack(&file, &other).unwrap();
    let got = ProjectDoc::from_bytes(&b.doc, 2).unwrap();
    assert_eq!(got.get(), p);
    let a = &assets(&p)[0];
    assert_eq!(std::fs::read(other.asset(a)).unwrap(), b"backing audio");
    assert!(!other.dir.join("Exports").exists(), "exports stay behind");
    // Unpacking again keeps what is there.
    bundle::unpack(&file, &other).unwrap();
}

#[test]
fn importing_a_bundle_of_the_same_project_merges_it() {
    let dir = temp("merge");
    let (f, mut mine, p) = project(&dir);
    // Sent to a student, who adds a marker and sends it back.
    let file = dir.join("to-student.dawbundle");
    bundle::write(&file, "Song", &mine.to_bytes(), &f, &assets(&p)).unwrap();
    let theirs_dir = ProjectFolder::create(&dir.join("Student")).unwrap();
    let b = bundle::unpack(&file, &theirs_dir).unwrap();
    let mut theirs = ProjectDoc::from_bytes(&b.doc, 2).unwrap();
    let mut q = theirs.get();
    q.markers.push(daw_project::Marker {
        id: "m1".into(),
        bar: 9,
        name: "Solo".into(),
    });
    theirs.set(&q);
    // Meanwhile the teacher changes the tempo.
    let mut p2 = p.clone();
    p2.tempo_bpm = 90.0;
    mine.set(&p2);

    let back = dir.join("to-teacher.dawbundle");
    bundle::write(&back, "Song", &theirs.to_bytes(), &theirs_dir, &assets(&q)).unwrap();
    let b = bundle::unpack(&back, &f).unwrap();
    let incoming = ProjectDoc::from_bytes(&b.doc, 3).unwrap();
    assert!(daw_collab::related(&mine, &incoming));
    mine.merge(&b.doc).unwrap();
    let got = mine.get();
    assert_eq!(got.tempo_bpm, 90.0, "the teacher's change is kept");
    assert_eq!(got.markers.len(), 1, "the student's marker arrives");

    let mut stranger = ProjectDoc::new(9);
    stranger.set(&Project::default());
    assert!(!daw_collab::related(&mine, &stranger));
}

#[test]
fn damaged_or_foreign_bundles_are_refused() {
    let dir = temp("damaged");
    let (f, d, p) = project(&dir);
    let file = dir.join("Song.dawbundle");
    bundle::write(&file, "Song", &d.to_bytes(), &f, &assets(&p)).unwrap();
    let good = std::fs::read(&file).unwrap();
    let into = ProjectFolder::create(&dir.join("Into")).unwrap();

    let not_one = dir.join("notes.dawbundle");
    std::fs::write(&not_one, b"just some text").unwrap();
    assert!(bundle::peek(&not_one).is_err());

    // Cut short partway through the audio.
    let short = dir.join("short.dawbundle");
    std::fs::write(&short, &good[..good.len() - 4]).unwrap();
    assert!(bundle::unpack(&short, &into).is_err());

    // Audio that doesn't match its name.
    let mut bad = good.clone();
    let n = bad.len();
    bad[n - 1] ^= 0xff;
    let tampered = dir.join("tampered.dawbundle");
    std::fs::write(&tampered, &bad).unwrap();
    assert!(bundle::unpack(&tampered, &into).is_err());
    assert!(!into.has_asset(&assets(&p)[0]));

    // A name that tries to reach outside the folder.
    let evil = dir.join("evil.dawbundle");
    let mut e = Vec::new();
    e.extend_from_slice(b"DAWBUNDL");
    e.extend_from_slice(&1u32.to_le_bytes());
    e.extend_from_slice(&1u16.to_le_bytes());
    e.extend_from_slice(b"x");
    let doc = d.to_bytes();
    e.extend_from_slice(&(doc.len() as u64).to_le_bytes());
    e.extend_from_slice(&doc);
    e.extend_from_slice(&1u32.to_le_bytes());
    let name = b"../../escape.wav";
    e.extend_from_slice(&(name.len() as u16).to_le_bytes());
    e.extend_from_slice(name);
    e.extend_from_slice(&3u64.to_le_bytes());
    e.extend_from_slice(b"bad");
    std::fs::write(&evil, &e).unwrap();
    assert!(bundle::unpack(&evil, &into).is_err());
    assert!(!dir.join("escape.wav").exists());

    // A length far beyond the file.
    let mut huge = good[..12].to_vec();
    huge.extend_from_slice(&0u16.to_le_bytes());
    huge.extend_from_slice(&u64::MAX.to_le_bytes());
    let liar = dir.join("liar.dawbundle");
    std::fs::write(&liar, &huge).unwrap();
    assert!(bundle::peek(&liar).is_err());
}

#[test]
fn opening_merges_every_computers_copy() {
    let dir = temp("devices");
    let (f, mut a, p) = project(&dir);
    f.save_device("aaaa", &a.to_bytes()).unwrap();
    // Another computer opens it from the synced folder and edits.
    let (_, mut b, _) = daw_collab::open_doc(&f.dir, 2).unwrap();
    let mut q = b.get();
    q.notes = "From the other PC".into();
    b.set(&q);
    f.save_device("bbbb", &b.to_bytes()).unwrap();
    // And both saved project.daw: the last one wins, without b's edit.
    let mut p2 = p.clone();
    p2.tempo_bpm = 100.0;
    a.set(&p2);
    f.save(&a.to_bytes()).unwrap();
    f.save_device("aaaa", &a.to_bytes()).unwrap();

    let (_, got, failed) = daw_collab::open_doc(&f.dir, 3).unwrap();
    assert!(failed.is_empty());
    let got = got.get();
    assert_eq!(got.notes, "From the other PC");
    assert_eq!(got.tempo_bpm, 100.0);
}

fn wait_for(w: &FolderWatch, what: impl Fn(&Change) -> bool) -> Option<Change> {
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        if let Some(c) = w.poll().into_iter().find(|c| what(c)) {
            return Some(c);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

#[test]
fn the_watch_passes_on_other_computers_copies_and_new_audio() {
    let dir = temp("watch");
    let (f, a, _) = project(&dir);
    f.save_device("mine", &a.to_bytes()).unwrap();
    let w = FolderWatch::start(f.clone(), "mine", Duration::from_millis(20));
    f.save_device("theirs", &a.to_bytes()).unwrap();
    let got = wait_for(&w, |c| matches!(c, Change::Doc { .. }));
    let Some(Change::Doc { from, bytes }) = got else {
        panic!("the other copy was not passed on");
    };
    assert_eq!(from, f.device_file("theirs"));
    assert_eq!(bytes, a.to_bytes());
    // Ours is never passed on, and nothing again until it changes.
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        w.poll().iter().all(|c| !matches!(c, Change::Doc { .. })),
        "nothing new"
    );

    let wav = dir.join("new.wav");
    std::fs::write(&wav, b"more audio").unwrap();
    f.add_asset(&wav, false).unwrap();
    assert!(wait_for(&w, |c| matches!(c, Change::Audio)).is_some());
}
