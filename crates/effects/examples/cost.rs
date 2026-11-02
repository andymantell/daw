// SPDX-License-Identifier: GPL-3.0-or-later
//! How much of the audio thread each built-in effect takes: runs ten
//! seconds of noise through it at 48 kHz in 1440-frame buffers and
//! prints the time as a share of real time. Name an effect to run only
//! that one.
//!
//! `cargo run --release -p daw-effects --example cost -- [effect]`

use std::time::Instant;

use daw_effects::{Effect, Kind};
use daw_engine::graph::Insert;

const RATE: u32 = 48_000;
const BLOCK: usize = 1440;
const SECONDS: usize = 10;

fn main() {
    let only = std::env::args().nth(1);
    let mut seed = 1u32;
    let mut noise = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as f32 / (1 << 23) as f32 - 1.0
    };
    let input: Vec<f32> = (0..BLOCK).map(|_| noise() * 0.3).collect();
    for kind in Kind::ALL {
        let name = format!("{kind:?}");
        if only.as_ref().is_some_and(|o| !name.eq_ignore_ascii_case(o)) {
            continue;
        }
        let mut fx = Effect::new(kind).processor(RATE);
        let (mut left, mut right) = (input.clone(), input.clone());
        let blocks = SECONDS * RATE as usize / BLOCK;
        let start = Instant::now();
        for _ in 0..blocks {
            left.copy_from_slice(&input);
            right.copy_from_slice(&input);
            fx.process(&mut left, &mut right);
        }
        let used = start.elapsed().as_secs_f64() / SECONDS as f64;
        println!("{name:<12} {:>7.2}% of real time", used * 100.0);
    }
}
