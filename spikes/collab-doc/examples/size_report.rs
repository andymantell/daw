// SPDX-License-Identifier: GPL-3.0-or-later
//! Document size and load time for a large project (decision 0004
//! validation). Run with:
//! `cargo run --release -p spike-collab-doc --example size_report`

use spike_collab_doc::ProjectDoc;
use spike_collab_doc::loro_doc::LoroProject;
use spike_collab_doc::yrs_doc::YrsProject;
use std::time::Instant;

const TRACKS: usize = 200;
const CLIPS_PER_TRACK: usize = 20;
const EDITS: usize = 10_000;

fn report<D: ProjectDoc>(name: &str) {
    let mut d = D::new(1);
    let t0 = Instant::now();
    for t in 0..TRACKS {
        let id = format!("t{t}");
        d.add_track(&id, &format!("Track {t}"));
        for c in 0..CLIPS_PER_TRACK {
            d.add_clip(&id, &format!("t{t}c{c}"), (c * 3840) as i64, 3840);
        }
    }
    let build = t0.elapsed();
    let fresh_size = d.snapshot().len();

    // A long editing history: gain rides and clip nudges.
    let t1 = Instant::now();
    for i in 0..EDITS {
        let t = i % TRACKS;
        if i % 2 == 0 {
            d.set_gain(&format!("t{t}"), (i % 100) as f64 / 100.0);
        } else {
            let c = i % CLIPS_PER_TRACK;
            d.move_clip(&format!("t{t}"), &format!("t{t}c{c}"), (i * 10) as i64);
        }
    }
    let edit = t1.elapsed();
    let snap = d.snapshot();

    let t2 = Instant::now();
    let mut loaded = D::new(2);
    loaded.apply(&snap);
    let p = loaded.project();
    let load = t2.elapsed();
    assert_eq!(p.tracks.len(), TRACKS);

    // What one more small edit costs to send to a peer that is up to date.
    let v = loaded.version();
    d.set_gain("t0", 0.42);
    let one_edit = d.updates_since(&v).len();

    println!(
        "| {name} | {:.0} KB | {:.0} KB | {} B | {:.0} ms | {:.1} µs | {:.0} ms |",
        fresh_size as f64 / 1024.0,
        snap.len() as f64 / 1024.0,
        one_edit,
        build.as_secs_f64() * 1e3,
        edit.as_secs_f64() * 1e6 / EDITS as f64,
        load.as_secs_f64() * 1e3,
    );
}

fn main() {
    println!(
        "{TRACKS} tracks x {CLIPS_PER_TRACK} clips, then {EDITS} edits\n\n\
         | Library | Saved size, new | Saved size after edits | One edit sent to a peer | Build | Per edit | Load + read |\n\
         |---|---|---|---|---|---|---|"
    );
    report::<LoroProject>("Loro");
    report::<YrsProject>("yrs");
}
