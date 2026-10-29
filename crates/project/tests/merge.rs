// SPDX-License-Identifier: GPL-3.0-or-later
//! Scripted conflict tests (roadmap 1.9): people edit copies of one
//! project apart, swap them by each of the ways the app will, and every
//! copy must end up the same with nothing lost.
//!
//! Each test is a short script, one step per line:
//!
//! - `peers teacher student` starts each person with an empty copy.
//! - `teacher: add-track t1 Guitar` is an edit by one person (the edits
//!   are listed in [`edit`]).
//! - `live teacher student` swaps what is new both ways, as a live
//!   session does.
//! - `bundle teacher b1` keeps a whole copy of the teacher's project, as
//!   exporting a bundle does, and `open student b1` merges it in, so a
//!   bundle can arrive late, twice or never.
//! - `folder-save teacher` and `folder-open student` go through one shared
//!   project file on disk, as a cloud-synced folder does: saving merges
//!   what is in the file first, then writes. `folder-overwrite teacher`
//!   writes without merging, as when two machines save at once and the
//!   sync service keeps only one file.
//! - `reopen teacher` saves and reopens the project as a new run of the
//!   app.
//! - `expect …` checks one copy, or every copy for `*` (see [`expect`]);
//!   `expect same` checks that all copies agree.
//!
//! After the last step, every copy is synced with every other and must
//! agree. A seeded random script generator then runs many more scripts
//! and checks that nothing added is lost unless someone deleted it.
//!
//! The transports here are the document's own: the bundle file and
//! folder watching built on them are tested with those features.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use daw_project::{Clip, Effect, Marker, Project, ProjectDoc, Track};

struct Peer {
    name: String,
    id: u64,
    doc: ProjectDoc,
}

struct Run {
    peers: Vec<Peer>,
    bundles: BTreeMap<String, Vec<u8>>,
    folder: PathBuf,
    next_peer: u64,
}

impl Run {
    fn new(test: &str) -> Run {
        let folder = std::env::temp_dir().join(format!("daw-merge-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        Run {
            peers: Vec::new(),
            bundles: BTreeMap::new(),
            folder,
            next_peer: 1,
        }
    }

    fn peer(&mut self, name: &str) -> &mut Peer {
        self.peers
            .iter_mut()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("no peer called {name}"))
    }

    fn two(&mut self, a: &str, b: &str) -> (&mut Peer, &mut Peer) {
        let i = self.peers.iter().position(|p| p.name == a).unwrap();
        let j = self.peers.iter().position(|p| p.name == b).unwrap();
        assert_ne!(i, j);
        if i < j {
            let (x, y) = self.peers.split_at_mut(j);
            (&mut x[i], &mut y[0])
        } else {
            let (x, y) = self.peers.split_at_mut(i);
            (&mut y[0], &mut x[j])
        }
    }

    fn shared_file(&self) -> PathBuf {
        self.folder.join("project.daw")
    }

    fn new_peer_id(&mut self) -> u64 {
        self.next_peer += 1;
        self.next_peer
    }

    fn step(&mut self, line: &str) {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["peers", names @ ..] => {
                for n in names {
                    let id = self.new_peer_id();
                    self.peers.push(Peer {
                        name: n.to_string(),
                        id,
                        doc: ProjectDoc::new(id),
                    });
                }
            }
            ["live", a, b] => {
                let (a, b) = self.two(a, b);
                live(&mut a.doc, &mut b.doc);
            }
            ["bundle", p, name] => {
                let bytes = self.peer(p).doc.to_bytes();
                self.bundles.insert(name.to_string(), bytes);
            }
            ["open", p, name] => {
                let bytes = self.bundles[*name].clone();
                self.peer(p).doc.merge(&bytes).unwrap();
            }
            ["folder-save", p] => {
                let file = self.shared_file();
                let doc = &mut self.peer(p).doc;
                if let Ok(bytes) = std::fs::read(&file) {
                    doc.merge(&bytes).unwrap();
                }
                std::fs::write(&file, doc.to_bytes()).unwrap();
            }
            ["folder-overwrite", p] => {
                let file = self.shared_file();
                std::fs::write(&file, self.peer(p).doc.to_bytes()).unwrap();
            }
            ["folder-open", p] => {
                let bytes = std::fs::read(self.shared_file()).unwrap();
                self.peer(p).doc.merge(&bytes).unwrap();
            }
            ["reopen", p] => {
                let id = self.new_peer_id();
                let peer = self.peer(p);
                let bytes = peer.doc.to_bytes();
                peer.doc = ProjectDoc::from_bytes(&bytes, id).unwrap();
                peer.id = id;
            }
            ["expect", "same"] => self.expect_same(line),
            ["expect", who, what @ ..] => {
                let targets: Vec<usize> = if *who == "*" {
                    (0..self.peers.len()).collect()
                } else {
                    vec![self.peers.iter().position(|p| p.name == *who).unwrap()]
                };
                for i in targets {
                    let p = &self.peers[i];
                    if let Err(e) = expect(&p.doc.get(), what) {
                        panic!("{line}: {} has {e}", p.name);
                    }
                }
            }
            [who, rest @ ..] if who.ends_with(':') => {
                let doc = &mut self.peer(&who[..who.len() - 1]).doc;
                match rest {
                    ["undo"] => assert!(doc.undo(), "{line}: nothing to undo"),
                    ["redo"] => assert!(doc.redo(), "{line}: nothing to redo"),
                    _ => {
                        let mut p = doc.get();
                        edit(&mut p, rest).unwrap_or_else(|e| panic!("{line}: {e}"));
                        doc.set(&p);
                    }
                }
            }
            [] => {}
            _ => panic!("unknown step: {line}"),
        }
    }

    fn expect_same(&self, line: &str) {
        let first = self.peers[0].doc.get();
        for p in &self.peers[1..] {
            assert_eq!(
                p.doc.get(),
                first,
                "{line}: {} and {} differ",
                p.name,
                self.peers[0].name
            );
        }
    }

    /// Syncs every copy with every other.
    fn settle(&mut self) {
        let names: Vec<String> = self.peers.iter().map(|p| p.name.clone()).collect();
        for a in &names {
            for b in &names {
                if a != b {
                    let (x, y) = self.two(a, b);
                    live(&mut x.doc, &mut y.doc);
                }
            }
        }
        self.expect_same("after syncing everyone");
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

fn live(a: &mut ProjectDoc, b: &mut ProjectDoc) {
    let to_b = a.updates_since(&b.version()).unwrap();
    let to_a = b.updates_since(&a.version()).unwrap();
    b.merge(&to_b).unwrap();
    a.merge(&to_a).unwrap();
}

/// Runs a script; see the module comment.
fn run(test: &str, script: &str) {
    let mut r = Run::new(test);
    for line in script.lines() {
        let line = line.split('#').next().unwrap().trim();
        r.step(line);
    }
    r.settle();
}

// ---------------------------------------------------------------------------
// Edits and expectations

fn track<'a>(p: &'a mut Project, id: &str) -> Result<&'a mut Track, String> {
    p.tracks
        .iter_mut()
        .find(|t| t.id == id)
        .ok_or(format!("no track {id}"))
}

fn clip<'a>(p: &'a mut Project, t: &str, id: &str) -> Result<&'a mut Clip, String> {
    track(p, t)?
        .clips
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or(format!("no clip {id} on {t}"))
}

fn num<T: std::str::FromStr>(s: &str) -> Result<T, String> {
    s.parse().map_err(|_| format!("not a number: {s}"))
}

fn on(s: &str) -> Result<bool, String> {
    match s {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(format!("expected on or off, not {s}")),
    }
}

/// Applies one edit, as the app does: change the plain project, then hand
/// it to the document.
fn edit(p: &mut Project, words: &[&str]) -> Result<(), String> {
    let text = |w: &[&str]| w.join(" ");
    match words {
        ["add-track", id, name @ ..] => p.tracks.push(Track::new(id.to_string(), text(name))),
        ["rename-track", id, name @ ..] => track(p, id)?.name = text(name),
        ["delete-track", id] => p.tracks.retain(|t| t.id != *id),
        ["move-track", id, to] => {
            let i = p
                .tracks
                .iter()
                .position(|t| t.id == *id)
                .ok_or("no track")?;
            let t = p.tracks.remove(i);
            p.tracks.insert(num::<usize>(to)?.min(p.tracks.len()), t);
        }
        ["track-gain", id, db] => track(p, id)?.gain_db = num(db)?,
        ["mute", id, v] => track(p, id)?.mute = on(v)?,
        ["add-clip", t, id, start, len] => {
            let t = track(p, t)?;
            let seq = t.clips.iter().map(|c| c.seq + 1).max().unwrap_or(0);
            t.clips.push(Clip {
                id: id.to_string(),
                asset: format!("{id}.wav"),
                name: id.to_string(),
                start: num(start)?,
                offset: 0,
                len: num(len)?,
                take: None,
                muted: false,
                seq,
                ..Default::default()
            });
        }
        ["move-clip", t, id, start] => clip(p, t, id)?.start = num(start)?,
        ["trim-clip", t, id, offset, len] => {
            let c = clip(p, t, id)?;
            c.offset = num(offset)?;
            c.len = num(len)?;
        }
        ["delete-clip", t, id] => track(p, t)?.clips.retain(|c| c.id != *id),
        ["add-effect", t, id, name @ ..] => track(p, t)?.effects.push(Effect {
            id: id.to_string(),
            name: text(name),
            class: vec![1; 16],
            bundle: String::new(),
            bypass: false,
            state: None,
            ..Default::default()
        }),
        ["bypass", t, id, v] => {
            track(p, t)?
                .effects
                .iter_mut()
                .find(|e| e.id == *id)
                .ok_or("no effect")?
                .bypass = on(v)?
        }
        ["add-marker", id, bar, name @ ..] => {
            p.markers.push(Marker {
                id: id.to_string(),
                bar: num(bar)?,
                name: text(name),
            });
            p.markers.sort_by_key(|m| m.bar);
        }
        ["rename-marker", id, name @ ..] => {
            p.markers
                .iter_mut()
                .find(|m| m.id == *id)
                .ok_or("no marker")?
                .name = text(name)
        }
        ["delete-marker", id] => p.markers.retain(|m| m.id != *id),
        ["tempo", bpm] => p.tempo_bpm = num(bpm)?,
        ["loop", from, to] => p.loop_bars = (num(from)?, num(to)?),
        ["note", line @ ..] => {
            if !p.notes.is_empty() {
                p.notes.push('\n');
            }
            p.notes.push_str(&text(line));
        }
        ["note-first", line @ ..] => p.notes = format!("{}\n{}", text(line), p.notes),
        _ => return Err(format!("unknown edit: {}", text(words))),
    }
    Ok(())
}

/// Checks one copy. `Err` says what it has instead.
fn expect(p: &Project, words: &[&str]) -> Result<(), String> {
    let text = |w: &[&str]| w.join(" ");
    let find_track = |id: &str| p.tracks.iter().find(|t| t.id == id);
    let check = |ok: bool, got: String| if ok { Ok(()) } else { Err(got) };
    match words {
        ["tracks", ids @ ..] => {
            let got: Vec<&str> = p.tracks.iter().map(|t| t.id.as_str()).collect();
            check(got == ids, format!("tracks {got:?}"))
        }
        ["track", id, "name", name @ ..] => {
            let t = find_track(id).ok_or(format!("no track {id}"))?;
            check(t.name == text(name), format!("name {:?}", t.name))
        }
        ["track", id, "gain", db] => {
            let t = find_track(id).ok_or(format!("no track {id}"))?;
            check(t.gain_db == num::<f64>(db)?, format!("gain {}", t.gain_db))
        }
        ["track", id, "mute", v] => {
            let t = find_track(id).ok_or(format!("no track {id}"))?;
            check(t.mute == on(v)?, format!("mute {}", t.mute))
        }
        ["clips", t, ids @ ..] => {
            let t = find_track(t).ok_or(format!("no track {t}"))?;
            let mut got: Vec<&str> = t.clips.iter().map(|c| c.id.as_str()).collect();
            got.sort();
            let mut want = ids.to_vec();
            want.sort();
            check(got == want, format!("clips {got:?}"))
        }
        ["clip", t, id, "at", start, "len", len] => {
            let t = find_track(t).ok_or(format!("no track {t}"))?;
            let c = t.clips.iter().find(|c| c.id == *id).ok_or("no clip")?;
            check(
                c.start == num::<i64>(start)? && c.len == num::<i64>(len)?,
                format!("clip {id} at {} len {}", c.start, c.len),
            )
        }
        ["effects", t, ids @ ..] => {
            let t = find_track(t).ok_or(format!("no track {t}"))?;
            let got: Vec<&str> = t.effects.iter().map(|e| e.id.as_str()).collect();
            check(got == ids, format!("effects {got:?}"))
        }
        ["effect", t, id, "bypass", v] => {
            let t = find_track(t).ok_or(format!("no track {t}"))?;
            let e = t.effects.iter().find(|e| e.id == *id).ok_or("no effect")?;
            check(e.bypass == on(v)?, format!("bypass {}", e.bypass))
        }
        ["markers", ids @ ..] => {
            let got: Vec<&str> = p.markers.iter().map(|m| m.id.as_str()).collect();
            check(got == ids, format!("markers {got:?}"))
        }
        ["marker", id, "name", name @ ..] => {
            let m = p.markers.iter().find(|m| m.id == *id).ok_or("no marker")?;
            check(m.name == text(name), format!("marker name {:?}", m.name))
        }
        ["tempo", bpm] => check(
            p.tempo_bpm == num::<f64>(bpm)?,
            format!("tempo {}", p.tempo_bpm),
        ),
        ["tempo-one-of", bpms @ ..] => {
            let ok = bpms.iter().any(|b| b.parse::<f64>() == Ok(p.tempo_bpm));
            check(ok, format!("tempo {}", p.tempo_bpm))
        }
        ["loop", from, to] => check(
            p.loop_bars == (num(from)?, num(to)?),
            format!("loop {:?}", p.loop_bars),
        ),
        ["notes", lines @ ..] => {
            let want = text(lines).replace(" / ", "\n");
            check(p.notes == want, format!("notes {:?}", p.notes))
        }
        _ => Err(format!("an unknown expectation: {}", text(words))),
    }
}

// ---------------------------------------------------------------------------
// Scripts

/// The lesson week: the teacher sends the lesson as a bundle, both work
/// on it apart during the week, and the student sends it back.
#[test]
fn a_week_between_lessons_by_bundle() {
    run(
        "week",
        "
        peers teacher student
        teacher: add-track t1 Backing
        teacher: add-clip t1 c1 0 480000
        teacher: add-marker m1 1 Intro
        teacher: add-marker m2 9 Verse
        teacher: note Learn the verse riff.
        bundle teacher lesson1
        open student lesson1
        expect same

        # The week, apart.
        student: add-track t2 My take
        student: add-clip t2 c2 96000 48000
        student: note Bar 12 is hard.
        student: loop 9 13
        teacher: add-track t3 Click guide
        teacher: rename-marker m2 Verse 1
        teacher: note-first Warm up first.
        teacher: track-gain t1 -3

        bundle student back
        open teacher back
        expect teacher tracks t1 t3 t2
        expect teacher clips t2 c2
        expect teacher marker m2 name Verse 1
        expect teacher loop 9 13
        expect teacher track t1 gain -3
        expect teacher notes Warm up first. / Learn the verse riff. / Bar 12 is hard.
        bundle teacher lesson2
        open student lesson2
        expect same
        ",
    );
}

/// Opening the same bundle twice, or an old one after a newer, changes
/// nothing and undoes nothing.
#[test]
fn old_and_repeated_bundles_change_nothing() {
    run(
        "stale",
        "
        peers teacher student
        teacher: add-track t1 Guitar
        teacher: tempo 90
        bundle teacher old
        teacher: tempo 100
        teacher: rename-track t1 Lead guitar
        teacher: add-clip t1 c1 0 1000
        bundle teacher new
        open student new
        open student new
        open student old
        expect student tempo 100
        expect student track t1 name Lead guitar
        expect student clips t1 c1
        open teacher old
        expect same
        ",
    );
}

/// Different fields of one clip changed by each side both survive; the
/// same field changed by both ends the same on every copy.
#[test]
fn edits_to_one_clip_and_one_field() {
    run(
        "fields",
        "
        peers a b
        a: add-track t1 Bass
        a: add-clip t1 c1 0 48000
        live a b
        a: move-clip t1 c1 24000
        b: trim-clip t1 c1 0 12000
        a: tempo 100
        b: tempo 110
        a: mute t1 on
        live a b
        expect * clip t1 c1 at 24000 len 12000
        expect * tempo-one-of 100 110
        expect * track t1 mute on
        expect same
        ",
    );
}

/// Deleting wins over a concurrent edit to the deleted thing; nothing
/// else is touched.
#[test]
fn delete_against_edit() {
    run(
        "delete",
        "
        peers a b
        a: add-track t1 Guitar
        a: add-track t2 Bass
        a: add-clip t1 c1 0 1000
        a: add-clip t1 c2 2000 1000
        a: add-marker m1 5 Chorus
        live a b
        a: delete-clip t1 c1
        b: move-clip t1 c1 500
        b: move-clip t1 c2 2500
        a: delete-track t2
        b: rename-track t2 Bass DI
        b: delete-marker m1
        a: rename-marker m1 Big chorus
        live a b
        expect * clips t1 c2
        expect * clip t1 c2 at 2500 len 1000
        expect * tracks t1
        expect * markers
        ",
    );
}

/// Track order: one side reorders while the other adds and removes.
#[test]
fn reordering_while_adding() {
    run(
        "order",
        "
        peers a b
        a: add-track t1 One
        a: add-track t2 Two
        a: add-track t3 Three
        live a b
        a: move-track t3 0
        b: add-track t4 Four
        b: move-track t1 2
        live a b
        expect same
        a: delete-track t2
        b: move-track t2 0
        b: add-track t5 Five
        live a b
        expect * tracks t3 t1 t4 t5
        ",
    );
}

/// Effects added on one side and bypassed on the other.
#[test]
fn effects_added_and_bypassed_apart() {
    run(
        "effects",
        "
        peers a b
        a: add-track t1 Guitar
        a: add-effect t1 e1 EQ
        live a b
        a: add-effect t1 e2 Compressor
        b: bypass t1 e1 on
        b: add-effect t1 e3 Reverb
        live a b
        expect * effect t1 e1 bypass on
        expect same
        ",
    );
}

/// Undo after merging takes back only your own edit, and a later sync
/// carries the undo to the other side.
#[test]
fn undo_after_merging() {
    run(
        "undo",
        "
        peers teacher student
        teacher: add-track t1 Guitar
        live teacher student
        teacher: tempo 80
        student: rename-track t1 Rhythm
        live teacher student
        teacher: undo
        expect teacher tempo 120
        expect teacher track t1 name Rhythm
        live teacher student
        expect student tempo 120
        student: undo
        live teacher student
        expect * track t1 name Guitar
        expect * tempo 120
        ",
    );
}

/// A shared folder: each side saves into one file, merging what is
/// there first. Even when one save overwrites the other's, nothing is
/// lost, because the overwritten side still has its edits and the next
/// save merges them back.
#[test]
fn a_shared_folder_with_saves_crossing() {
    run(
        "folder",
        "
        peers home school
        home: add-track t1 Piano
        folder-save home
        folder-open school
        school: add-track t2 Voice
        home: add-clip t1 c1 0 48000
        folder-save school
        folder-save home
        folder-open school
        expect same
        expect * tracks t1 t2

        # Both save at once; the sync service keeps home's file.
        school: add-marker m1 3 Bridge
        home: note Slow the bridge down.
        folder-overwrite school
        folder-overwrite home
        folder-open school
        expect school markers m1
        expect school notes Slow the bridge down.
        folder-save school
        folder-open home
        expect home markers m1
        expect same
        ",
    );
}

/// Three people by three routes: teacher and student live, the student's
/// second machine through a shared folder, and a parent's laptop by
/// bundle, with a restart in the middle.
#[test]
fn three_routes_at_once() {
    run(
        "routes",
        "
        peers teacher student laptop
        teacher: add-track t1 Backing
        teacher: add-marker m1 1 Intro
        live teacher student
        folder-save student
        folder-open laptop
        laptop: add-track t2 Practice take
        laptop: add-clip t2 c1 0 48000
        teacher: add-track t3 Teacher's part
        student: note Use the bridge pickup.
        reopen laptop
        laptop: rename-track t2 Practice take 2
        folder-save laptop
        live teacher student
        folder-open student
        live teacher student
        bundle teacher evening
        open laptop evening
        expect same
        expect * tracks t1 t3 t2
        expect * track t2 name Practice take 2
        expect * notes Use the bridge pickup.
        ",
    );
}

// ---------------------------------------------------------------------------
// Random scripts

/// xorshift64*, seeded, so a failing script can be run again.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a, T>(&mut self, from: &'a [T]) -> Option<&'a T> {
        (!from.is_empty()).then(|| &from[self.below(from.len())])
    }
}

/// What a random script added and deleted, to check the result against.
#[derive(Default)]
struct Ledger {
    tracks: BTreeSet<String>,
    deleted_tracks: BTreeSet<String>,
    /// Clip id → its track.
    clips: BTreeMap<String, String>,
    deleted_clips: BTreeSet<String>,
    markers: BTreeSet<String>,
    deleted_markers: BTreeSet<String>,
    notes: Vec<String>,
}

/// One random step for one of three peers: an edit on what that peer can
/// see, or a sync by a random route.
fn random_step(rng: &mut Rng, r: &Run, ledger: &mut Ledger, n: usize) -> String {
    let names = ["a", "b", "c"];
    let who = names[rng.below(3)];
    let other = loop {
        let o = names[rng.below(3)];
        if o != who {
            break o;
        }
    };
    let p = r.peers.iter().find(|p| p.name == who).unwrap().doc.get();
    let tracks: Vec<String> = p.tracks.iter().map(|t| t.id.clone()).collect();
    let clips: Vec<(String, String)> = p
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter().map(|c| (t.id.clone(), c.id.clone())))
        .collect();
    let markers: Vec<String> = p.markers.iter().map(|m| m.id.clone()).collect();
    match rng.below(14) {
        0 => format!("live {who} {other}"),
        1 => {
            let b = format!("b{n}");
            format!("bundle {who} {b}\nopen {other} {b}")
        }
        2 => format!("folder-save {who}"),
        3 if r.shared_file().exists() => format!("folder-open {who}"),
        4 => {
            let id = format!("t{n}");
            ledger.tracks.insert(id.clone());
            format!("{who}: add-track {id} Track {n}")
        }
        5 if !tracks.is_empty() && rng.below(3) == 0 => {
            let t = rng.pick(&tracks).unwrap();
            ledger.deleted_tracks.insert(t.clone());
            format!("{who}: delete-track {t}")
        }
        6 if !tracks.is_empty() => {
            let t = rng.pick(&tracks).unwrap();
            format!("{who}: move-track {t} {}", rng.below(tracks.len()))
        }
        7 if !tracks.is_empty() => {
            let t = rng.pick(&tracks).unwrap();
            format!("{who}: rename-track {t} Name {n}")
        }
        8 if !tracks.is_empty() => {
            let t = rng.pick(&tracks).unwrap().clone();
            let id = format!("c{n}");
            ledger.clips.insert(id.clone(), t.clone());
            format!("{who}: add-clip {t} {id} {} 1000", rng.below(100) * 480)
        }
        9 if !clips.is_empty() => {
            let (t, c) = rng.pick(&clips).unwrap();
            format!("{who}: move-clip {t} {c} {}", rng.below(100) * 480)
        }
        10 if !clips.is_empty() && rng.below(2) == 0 => {
            let (t, c) = rng.pick(&clips).unwrap();
            ledger.deleted_clips.insert(c.clone());
            format!("{who}: delete-clip {t} {c}")
        }
        11 => {
            let id = format!("m{n}");
            ledger.markers.insert(id.clone());
            format!("{who}: add-marker {id} {} Part {n}", 1 + rng.below(32))
        }
        12 if !markers.is_empty() && rng.below(2) == 0 => {
            let m = rng.pick(&markers).unwrap();
            ledger.deleted_markers.insert(m.clone());
            format!("{who}: delete-marker {m}")
        }
        13 => {
            let line = format!("[line{n}]");
            ledger.notes.push(line.clone());
            format!("{who}: note {line}")
        }
        _ => format!("{who}: tempo {}", 60 + rng.below(120)),
    }
}

/// Many random scripts on three peers. After everyone syncs, every copy
/// agrees, and everything added is there exactly once unless someone
/// deleted it (or the track it was on).
#[test]
fn random_scripts_lose_nothing() {
    for seed in 1..=100u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let mut r = Run::new(&format!("random-{seed}"));
        r.step("peers a b c");
        let mut ledger = Ledger::default();
        let mut script = String::from("peers a b c\n");
        for n in 0..50 {
            let step = random_step(&mut rng, &r, &mut ledger, n);
            for line in step.lines() {
                script.push_str(line);
                script.push('\n');
                r.step(line);
            }
        }
        r.settle();
        let p = r.peers[0].doc.get();
        let fail = |what: String| panic!("seed {seed}: {what}\nscript:\n{script}");

        let got: Vec<&String> = p.tracks.iter().map(|t| &t.id).collect();
        let unique: BTreeSet<&String> = got.iter().copied().collect();
        if unique.len() != got.len() {
            fail(format!("a track appears twice: {got:?}"));
        }
        let want: BTreeSet<&String> = ledger.tracks.difference(&ledger.deleted_tracks).collect();
        if unique != want {
            fail(format!("tracks {unique:?}, expected {want:?}"));
        }
        let mut got_clips = BTreeSet::new();
        for t in &p.tracks {
            for c in &t.clips {
                if !got_clips.insert(c.id.clone()) {
                    fail(format!("clip {} appears twice", c.id));
                }
            }
        }
        let want_clips: BTreeSet<String> = ledger
            .clips
            .iter()
            .filter(|(c, t)| !ledger.deleted_clips.contains(*c) && want.contains(t))
            .map(|(c, _)| c.clone())
            .collect();
        if got_clips != want_clips {
            fail(format!("clips {got_clips:?}, expected {want_clips:?}"));
        }
        let got_markers: BTreeSet<String> = p.markers.iter().map(|m| m.id.clone()).collect();
        let want_markers: BTreeSet<String> = ledger
            .markers
            .difference(&ledger.deleted_markers)
            .cloned()
            .collect();
        if got_markers != want_markers {
            fail(format!(
                "markers {got_markers:?}, expected {want_markers:?}"
            ));
        }
        for line in &ledger.notes {
            if p.notes.matches(line.as_str()).count() != 1 {
                fail(format!(
                    "note {line} is not there exactly once: {:?}",
                    p.notes
                ));
            }
        }
    }
}
