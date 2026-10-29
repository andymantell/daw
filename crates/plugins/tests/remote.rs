// SPDX-License-Identifier: GPL-3.0-or-later
//! Plugins in a helper process (roadmap 2.4): audio and settings go
//! through the helper as they would in the app, and a plugin that
//! crashes or hangs while processing takes down only the helper.

#[path = "common/bundle.rs"]
mod bundle;

use std::path::Path;
use std::time::{Duration, Instant};

use bundle::{broken, bundle};
use daw_engine::graph::Insert;
use daw_plugins::host::{HANG_TIMEOUT, Host};
use daw_plugins::{Plugin, PluginState, Processor};
use daw_test_plugin::{LATENCY, NAME, PROCESSOR_CID};

const RATE: f64 = 48_000.0;
const BLOCK: usize = 256;

fn helper() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_daw-plugin-helper"))
}

fn spawn() -> Host {
    Host::spawn(helper()).unwrap()
}

/// Runs `blocks` blocks of ones through `fx` and returns the left
/// channel of the last.
fn ones(fx: &mut impl Insert, blocks: usize) -> Vec<f32> {
    let mut a = Vec::new();
    for _ in 0..blocks {
        a = vec![1.0; BLOCK];
        let mut b = vec![1.0; BLOCK];
        fx.process(&mut a, &mut b);
    }
    a
}

#[test]
fn a_plugin_in_a_helper_processes_audio_and_keeps_its_settings() {
    let host = spawn();
    let (plugin, mut fx) = host
        .instantiate(&bundle(), &PROCESSOR_CID, RATE, BLOCK, None)
        .unwrap();
    let plugin = Plugin::Remote(plugin);
    assert_eq!(plugin.latency(), LATENCY);
    let params = plugin.params();
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].title, "Gain");
    let id = params[0].id;
    assert_eq!(plugin.param(id), Some(1.0));
    assert_eq!(plugin.param_text(id, 0.5).as_deref(), Some("0.500"));

    // Offline: the answer comes back with the block, after the plugin's
    // own delay.
    let out = ones(&mut fx, 1);
    assert!(out[LATENCY as usize..].iter().all(|&x| x == 1.0), "{out:?}");

    plugin.set_param(id, 0.25);
    assert_eq!(plugin.param(id), Some(0.25));
    let out = ones(&mut fx, 2);
    assert!(out.iter().all(|&x| x == 0.25), "{out:?}");

    // Settings survive a save and load, and a damaged load is refused.
    let state = plugin.save_state().unwrap();
    plugin.set_param(id, 0.75);
    plugin.load_state(&state).unwrap();
    assert_eq!(plugin.param(id), Some(0.25));
    assert!(
        plugin
            .load_state(&PluginState {
                component: vec![1],
                controller: None,
            })
            .is_err()
    );
    assert_eq!(plugin.param(id), Some(0.25));
    assert!(plugin.failure().is_none());
    assert!(plugin.programs().is_empty());
}

#[test]
fn a_live_plugin_runs_one_block_behind_and_never_waits() {
    let host = spawn();
    let (mut plugin, mut fx) = host
        .instantiate(&bundle(), &PROCESSOR_CID, RATE, BLOCK, Some(BLOCK))
        .unwrap();
    // The extra block is reported as latency.
    assert_eq!(plugin.latency(), LATENCY + BLOCK as u32);
    assert_eq!(plugin.poll().latency, LATENCY + BLOCK as u32);

    // Paced like an audio device, the ones come back one block (plus
    // the plugin's delay) later, and no block is late.
    let mut out = Vec::new();
    for _ in 0..8 {
        let (mut a, mut b) = (vec![1.0; BLOCK], vec![1.0; BLOCK]);
        fx.process(&mut a, &mut b);
        out.extend(a);
        std::thread::sleep(Duration::from_millis(6));
    }
    let delay = BLOCK + LATENCY as usize;
    assert!(out[..delay].iter().all(|&x| x == 0.0));
    assert!(
        out[delay..].iter().all(|&x| x == 1.0),
        "{:?}",
        &out[delay..]
    );
    if let Processor::Remote(p) = &Processor::Remote(fx) {
        assert_eq!(p.late(), 0);
    }
    assert_eq!(plugin.late(), 0);
}

#[test]
fn a_plugin_that_crashes_while_processing_takes_only_the_helper() {
    let crash = broken("DawTestCrashProcess", "crash-on-process");
    let host = spawn();
    let (good, mut good_fx) = host
        .instantiate(&bundle(), &PROCESSOR_CID, RATE, BLOCK, None)
        .unwrap();
    let (bad, mut bad_fx) = host
        .instantiate(&crash, &PROCESSOR_CID, RATE, BLOCK, None)
        .unwrap();
    assert!(bad.failure().is_none());

    // The crash: audio passes through unchanged, and both plugins in the
    // helper report it.
    let (mut a, mut b) = (vec![0.5; BLOCK], vec![0.5; BLOCK]);
    bad_fx.process(&mut a, &mut b);
    assert!(a.iter().all(|&x| x == 0.5));
    let why = bad.failure().expect("the crash is reported");
    assert!(why.contains("crashed"), "{why}");
    assert_eq!(good.failure(), Some(why.clone()));
    assert_eq!(host.failure(), Some(why));
    let (mut a, mut b) = (vec![0.5; BLOCK], vec![0.5; BLOCK]);
    good_fx.process(&mut a, &mut b);
    assert!(a.iter().all(|&x| x == 0.5));
    assert!(good.save_state().is_err());
    assert_eq!(good.param(0), None);

    // A new helper works as before.
    let again = spawn();
    let (p, mut fx) = again
        .instantiate(&bundle(), &PROCESSOR_CID, RATE, BLOCK, None)
        .unwrap();
    assert!(ones(&mut fx, 2).iter().all(|&x| x == 1.0));
    assert!(p.failure().is_none());
}

#[test]
fn a_live_plugin_whose_helper_crashes_goes_silent_then_passes_through() {
    let crash = broken("DawTestCrashProcess", "crash-on-process");
    let host = spawn();
    let (plugin, mut fx) = host
        .instantiate(&crash, &PROCESSOR_CID, RATE, BLOCK, Some(BLOCK))
        .unwrap();
    let start = Instant::now();
    while plugin.failure().is_none() && start.elapsed() < Duration::from_secs(10) {
        let (mut a, mut b) = (vec![1.0; BLOCK], vec![1.0; BLOCK]);
        fx.process(&mut a, &mut b);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(plugin.failure().is_some());
    let (mut a, mut b) = (vec![1.0; BLOCK], vec![1.0; BLOCK]);
    fx.process(&mut a, &mut b);
    assert!(a.iter().all(|&x| x == 1.0));
}

#[test]
fn a_plugin_that_hangs_while_processing_is_stopped() {
    let hang = broken("DawTestHangProcess", "hang-on-process");
    let host = spawn();
    let (plugin, mut fx) = host
        .instantiate(&hang, &PROCESSOR_CID, RATE, BLOCK, None)
        .unwrap();
    let start = Instant::now();
    let (mut a, mut b) = (vec![0.5; BLOCK], vec![0.5; BLOCK]);
    fx.process(&mut a, &mut b);
    let took = start.elapsed();
    assert!(took >= HANG_TIMEOUT && took < HANG_TIMEOUT * 3, "{took:?}");
    assert!(a.iter().all(|&x| x == 0.5));
    let why = plugin.failure().unwrap();
    assert!(why.contains("stopped answering"), "{why}");
}

#[test]
fn a_helper_that_cannot_load_a_plugin_says_why_and_carries_on() {
    let refuse = broken("DawTestRefuse", "refuse-setup");
    let host = spawn();
    let err = host
        .instantiate(&refuse, &PROCESSOR_CID, RATE, BLOCK, None)
        .err()
        .unwrap();
    assert!(err.contains("setupProcessing"), "{err}");
    let err = host
        .instantiate(
            Path::new("/nowhere.vst3"),
            &PROCESSOR_CID,
            RATE,
            BLOCK,
            None,
        )
        .err()
        .unwrap();
    assert!(err.contains("could not load"), "{err}");
    assert!(host.failure().is_none());
    let (p, _fx) = host
        .instantiate(&bundle(), &PROCESSOR_CID, RATE, BLOCK, None)
        .unwrap();
    assert_eq!(p.params()[0].title, "Gain");
    let _ = NAME;
}
