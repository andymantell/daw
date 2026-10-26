// SPDX-License-Identifier: GPL-3.0-or-later
//! Spike 0.1 D pass criteria, checked without an audio device.

use spike_engine::{Command, Graph, Osc, SwapMode, rt_check};
use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

const SR: f32 = 48_000.0;
const BLOCK: usize = 128;
/// Largest sample-to-sample step a glitch-free output may take. Graphs in
/// these tests hold at most four oscillators of gain 0.25 at up to 1 kHz,
/// so the natural slew is at most 2π·1000/48000 ≈ 0.131. A crossfade adds
/// at most 2/128 per sample.
const MAX_STEP: f32 = 0.2;

/// Small deterministic PRNG, so the test needs no extra crates.
struct XorShift(u64);
impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

fn random_graph(rng: &mut XorShift) -> Graph {
    let n = 1 + (rng.next() % 4) as usize;
    Graph::new(
        (0..n)
            .map(|_| Osc::new(50.0 + rng.unit() * 950.0, rng.unit() * 0.25))
            .collect(),
    )
}

/// Runs `blocks` blocks on a dedicated "audio" thread and returns
/// (allocator calls on that thread, largest sample step).
fn run_audio(
    mut engine: spike_engine::Engine,
    blocks: usize,
    stop: Arc<AtomicBool>,
) -> (usize, f32) {
    thread::spawn(move || {
        let mut out = vec![0.0f32; BLOCK];
        let mut last = 0.0f32;
        let mut max_step = 0.0f32;
        {
            let _rt = rt_check::RtGuard::enter();
            for _ in 0..blocks {
                engine.process(&mut out);
                for &s in &out {
                    max_step = max_step.max((s - last).abs());
                    last = s;
                }
            }
        }
        stop.store(true, Ordering::Release);
        let v = rt_check::violations();
        // Engine (and its buffers) are dropped here, outside the guard.
        drop(engine);
        (v, max_step)
    })
    .join()
    .unwrap()
}

#[test]
fn concurrent_commands_and_swaps_are_glitch_free_and_allocation_free() {
    let mut rng = XorShift(0x5eed);
    let (mut ctl, engine) = spike_engine::new(random_graph(&mut rng), SR, SwapMode::Crossfade);
    let stop = Arc::new(AtomicBool::new(false));

    // UI thread: hammer the engine with gain changes and graph swaps.
    let ui_stop = stop.clone();
    let ui = thread::spawn(move || {
        let (mut sent, mut swaps, mut freed) = (0usize, 0usize, 0usize);
        while !ui_stop.load(Ordering::Acquire) {
            let cmd = if rng.next().is_multiple_of(8) {
                swaps += 1;
                Command::SwapGraph(Box::new(random_graph(&mut rng)))
            } else {
                Command::SetGain {
                    osc: (rng.next() % 4) as usize,
                    gain: rng.unit() * 0.25,
                }
            };
            if ctl.send(cmd).is_ok() {
                sent += 1;
            }
            freed += ctl.collect_garbage();
            thread::yield_now();
        }
        freed += ctl.collect_garbage();
        (sent, swaps, freed)
    });

    // 30 minutes of audio at 48 kHz, rendered as fast as possible.
    let blocks = (30.0 * 60.0 * SR) as usize / BLOCK;
    let (violations, max_step) = run_audio(engine, blocks, stop);
    let (sent, swaps, freed) = ui.join().unwrap();

    eprintln!(
        "blocks={blocks} commands={sent} swaps≈{swaps} graphs freed on UI thread={freed} max_step={max_step:.4}"
    );
    assert_eq!(violations, 0, "audio thread touched the allocator");
    assert!(max_step < MAX_STEP, "glitch: step {max_step}");
    assert!(freed > 0, "no graphs came back for freeing");
}

fn dc_graph() -> Graph {
    // 0 Hz with phase π/2: a constant 1.0, so any jump comes from the swap.
    Graph::new(vec![Osc::with_phase(0.0, 1.0, FRAC_PI_2)])
}

fn step_across_swap(mode: SwapMode) -> f32 {
    let (mut ctl, engine) = spike_engine::new(dc_graph(), SR, mode);
    let mut engine = engine;
    let mut out = vec![0.0f32; BLOCK];
    engine.process(&mut out);
    let mut last = *out.last().unwrap();
    assert!((last - 1.0).abs() < 1e-6);
    ctl.send(Command::SwapGraph(Box::default())).ok().unwrap();
    let mut max_step = 0.0f32;
    for _ in 0..2 {
        engine.process(&mut out);
        for &s in &out {
            max_step = max_step.max((s - last).abs());
            last = s;
        }
    }
    max_step
}

#[test]
fn crossfade_removes_the_click_a_hard_swap_makes() {
    let hard = step_across_swap(SwapMode::Hard);
    let faded = step_across_swap(SwapMode::Crossfade);
    // The detector must see the hard swap's click...
    assert!(hard > MAX_STEP, "hard swap step {hard} not detected");
    // ...and the crossfade must remove it.
    assert!(faded <= 1.0 / BLOCK as f32 + 1e-6, "crossfade step {faded}");
}

#[test]
fn allocation_detector_catches_allocations() {
    thread::spawn(|| {
        let before = rt_check::violations();
        {
            let _rt = rt_check::RtGuard::enter();
            std::hint::black_box(vec![1u8; 16]);
        }
        assert!(rt_check::violations() > before);
    })
    .join()
    .unwrap();
}
