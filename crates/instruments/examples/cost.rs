// SPDX-License-Identifier: GPL-3.0-or-later
//! How much of the audio thread each built-in instrument takes: plays a
//! four-note chord for ten seconds at 48 kHz in 1440-frame buffers and
//! prints the time as a share of real time. Name an instrument to run
//! only that one.
//!
//! `cargo run --release -p daw-instruments --example cost -- [instrument]`

use std::time::Instant;

use daw_engine::midi::Event;
use daw_instruments::{Builtin, Kind};

const RATE: u32 = 48_000;
const BLOCK: usize = 1440;
const SECONDS: usize = 10;

fn main() {
    let filter = std::env::args().nth(1);
    for kind in Kind::ALL {
        let name = format!("{kind:?}");
        if filter
            .as_ref()
            .is_some_and(|f| !name.eq_ignore_ascii_case(f))
        {
            continue;
        }
        let mut inst = Builtin::new(kind).make(RATE);
        let mut left = vec![0.0; BLOCK];
        let mut right = vec![0.0; BLOCK];
        let chord: Vec<Event> = [57, 61, 64, 69]
            .iter()
            .enumerate()
            .map(|(i, &p)| Event::note_on(0, 0, p, 100, i as u32))
            .collect();
        let blocks = SECONDS * RATE as usize / BLOCK;
        let start = Instant::now();
        for b in 0..blocks {
            let events: &[Event] = if b == 0 { &chord } else { &[] };
            inst.process(events, &mut left, &mut right);
        }
        let used = start.elapsed().as_secs_f64() / SECONDS as f64;
        println!("{name:<12} {:>7.2}% of real time", used * 100.0);
    }
}
