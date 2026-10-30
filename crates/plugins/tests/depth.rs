// SPDX-License-Identifier: GPL-3.0-or-later
//! Plugin depth (roadmap 2.4), with the test plugin's router class: a
//! sidechain input and an extra output switched on by a layout, a delay
//! that changes while it plays, a newer version standing in for an
//! older one, and the checks behind the compatibility list. Each runs in
//! the app's own process and in a helper process, as the app may.

#[path = "common/bundle.rs"]
mod bundle;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bundle::{broken, bundle};
use daw_engine::graph::Insert;
use daw_plugins::check::{self, Outcome};
use daw_plugins::host::{Host, RemoteProcessor};
use daw_plugins::scan::{self, PluginList};
use daw_plugins::{Layout, Module, Plugin, Processor, instantiate_with};
use daw_test_plugin::{
    OLD_LOOKAHEAD_PARAM, OLD_ROUTER_CID, PROCESSOR_CID, ROUTER_CID, ROUTER_LOOKAHEAD, ROUTER_NAME,
};

const RATE: f64 = 48_000.0;
const BLOCK: usize = 64;

fn helper() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_daw-plugin-helper"))
}

fn both() -> Layout {
    Layout {
        sidechain: true,
        outputs: vec![1],
    }
}

/// The two sides of the router, in this process or in a helper (for
/// rendering, so each block comes straight back).
fn router(remote: bool, layout: &Layout) -> (Plugin, Processor, Option<Host>) {
    if remote {
        let host = Host::spawn(helper()).unwrap();
        let (p, x) = host
            .instantiate_with(&bundle(), &ROUTER_CID, RATE, BLOCK, None, layout)
            .unwrap();
        (Plugin::Remote(p), Processor::Remote(x), Some(host))
    } else {
        let module = Arc::new(Module::load(&bundle()).unwrap());
        let (p, x) = instantiate_with(module, &ROUTER_CID, RATE, BLOCK, layout).unwrap();
        (Plugin::Local(p), Processor::Local(x), None)
    }
}

/// One block: the track's audio all `main`, its sidechain all `key`.
/// The left channel that comes out.
fn block(fx: &mut impl Insert, main: f32, key: Option<f32>) -> Vec<f32> {
    let (mut l, mut r) = (vec![main; BLOCK], vec![main; BLOCK]);
    match key {
        Some(k) => {
            let k = vec![k; BLOCK];
            fx.process_keyed(&mut l, &mut r, (&k, &k));
        }
        None => fx.process(&mut l, &mut r),
    }
    assert_eq!(l, r);
    l
}

#[test]
fn the_buses_a_layout_asks_for_are_switched_on() {
    for remote in [false, true] {
        let (plugin, mut fx, _host) = router(remote, &Layout::default());
        let buses = plugin.buses();
        assert_eq!(buses.inputs.len(), 2);
        assert_eq!(buses.sidechain(), Some(1));
        assert_eq!(buses.inputs[1].name, "Sidechain");
        assert_eq!(buses.extra_outputs().count(), 1);
        assert!(buses.outputs.iter().all(|b| b.channels == 2));
        // Main buses only: the sidechain is not heard, even if offered.
        assert_eq!(block(&mut fx, 0.5, Some(0.25)), vec![0.5; BLOCK]);

        // The extra output (the sidechain, passed through) mixed in.
        let (_p, mut fx, _host) = router(remote, &both());
        assert_eq!(block(&mut fx, 0.5, Some(0.25)), vec![0.75; BLOCK]);
        // No sidechain this block: the side input is silent.
        assert_eq!(block(&mut fx, 0.5, None), vec![0.5; BLOCK]);

        // The sidechain on but the extra output not mixed in.
        let only_key = Layout {
            sidechain: true,
            outputs: vec![],
        };
        let (_p, mut fx, _host) = router(remote, &only_key);
        assert_eq!(
            block(&mut fx, 0.5, Some(0.25)),
            vec![0.5; BLOCK],
            "remote {remote}"
        );
    }
}

/// Sets "Look-ahead" and polls as the app does once a frame, until the
/// plugin says it was restarted.
fn look_ahead(plugin: &mut Plugin, on: bool) -> daw_plugins::proto::Status {
    plugin.set_param(0, if on { 1.0 } else { 0.0 });
    plugin.poll()
}

#[test]
fn a_delay_changed_while_playing_is_reported_once_restarted() {
    for remote in [false, true] {
        let (mut plugin, mut fx, _host) = router(remote, &Layout::default());
        assert_eq!(plugin.latency(), 0);
        assert_eq!(block(&mut fx, 1.0, None), vec![1.0; BLOCK]);

        let status = look_ahead(&mut plugin, true);
        assert!(status.restart, "remote {remote}");
        assert!(!status.reload);
        assert_eq!(status.latency, ROUTER_LOOKAHEAD, "remote {remote}");
        assert_eq!(plugin.latency(), ROUTER_LOOKAHEAD);
        // Restarted: the delay line starts empty, and the sound comes
        // out the new delay later.
        let out = block(&mut fx, 1.0, None);
        let n = ROUTER_LOOKAHEAD as usize;
        assert!(out[..n].iter().all(|&x| x == 0.0));
        assert!(out[n..].iter().all(|&x| x == 1.0));

        // Nothing more to report until it changes again.
        let status = plugin.poll();
        assert!(!status.restart);
        assert_eq!(look_ahead(&mut plugin, false).latency, 0);
    }
}

#[test]
fn a_restart_in_the_app_silences_the_audio_side_meanwhile() {
    // The audio thread never waits for a restart: a block that comes
    // while one is under way is silent instead.
    let module = Arc::new(Module::load(&bundle()).unwrap());
    let (plugin, fx) =
        instantiate_with(module, &ROUTER_CID, RATE, BLOCK, &Layout::default()).unwrap();
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let stop = Arc::new(AtomicBool::new(false));
    let blocks = Arc::new(AtomicUsize::new(0));
    let audio = {
        let (stop, blocks) = (stop.clone(), blocks.clone());
        let mut fx = fx;
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let out = block(&mut fx, 1.0, None);
                // Each block is whole: all sound, or all silence.
                assert!(out.iter().all(|&x| x == 1.0) || out.iter().all(|&x| x == 0.0));
                blocks.fetch_add(1, Ordering::Relaxed);
            }
        })
    };
    let start = Instant::now();
    let mut restarts = 0;
    while restarts < 50 || blocks.load(Ordering::Relaxed) < 100 {
        assert!(start.elapsed() < Duration::from_secs(20));
        plugin.reactivate();
        restarts += 1;
    }
    stop.store(true, Ordering::Relaxed);
    audio.join().unwrap();
}

#[test]
fn a_newer_version_stands_in_for_an_older_one() {
    let classes = scan::scan_here(&bundle()).unwrap();
    let class = classes.iter().find(|c| c.cid == ROUTER_CID).unwrap();
    assert_eq!(class.replaces, vec![OLD_ROUTER_CID]);
    // Nothing else says it replaces anything.
    assert!(
        (classes.iter())
            .filter(|c| c.cid != ROUTER_CID)
            .all(|c| c.replaces.is_empty())
    );

    // Through a child process and the saved list, as the app scans.
    let dirs = vec![bundle().parent().unwrap().to_owned()];
    let mut list = PluginList::default();
    list.rescan(
        &dirs,
        |p| scan::scan_in_child(helper(), p, Duration::from_secs(60)),
        |_| {},
    );
    let list = PluginList::from_text(&list.to_text());
    let found = list.replacement_for(&OLD_ROUTER_CID).unwrap();
    assert_eq!(found.class.name, ROUTER_NAME);
    assert!(
        list.replacement_for(&PROCESSOR_CID).is_none(),
        "installed as itself"
    );
    let mut off = list.clone();
    off.set_enabled(&ROUTER_CID, false);
    assert!(off.replacement_for(&OLD_ROUTER_CID).is_none());

    // The older version's parameters map to the newer one's.
    for remote in [false, true] {
        let (plugin, _fx, _host) = router(remote, &Layout::default());
        assert_eq!(
            plugin.compatible_param(&OLD_ROUTER_CID, OLD_LOOKAHEAD_PARAM),
            Some(0)
        );
        assert_eq!(plugin.compatible_param(&OLD_ROUTER_CID, 99), None);
        assert_eq!(plugin.compatible_param(&PROCESSOR_CID, 7), None);
    }
}

#[test]
fn a_live_helper_carries_the_sidechain_beside_the_audio() {
    let host = Host::spawn(helper()).unwrap();
    let (_p, mut fx) = host
        .instantiate_with(&bundle(), &ROUTER_CID, RATE, BLOCK, Some(BLOCK), &both())
        .unwrap();
    // One block behind: the first comes back silent, the next carries
    // the first's audio and sidechain.
    let mut outs = Vec::new();
    for _ in 0..20 {
        outs.push(block(&mut fx, 0.5, Some(0.25)));
        let start = Instant::now();
        while let RemoteProcessor::Live(_) = &fx {
            if fx.available() >= BLOCK || start.elapsed() > Duration::from_secs(10) {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    assert_eq!(outs[0], vec![0.0; BLOCK]);
    assert_eq!(outs.last().unwrap(), &vec![0.75; BLOCK]);
    assert_eq!(fx.late(), 0);
}

#[test]
fn the_check_passes_good_plugins_and_notes_what_fails() {
    let classes = scan::scan_here(&bundle()).unwrap();
    let effects: Vec<_> = (classes.iter())
        .filter(|c| c.is_audio_plugin() && !c.is_instrument())
        .collect();
    let reports: Vec<_> = effects
        .iter()
        .map(|c| check::check(helper(), &bundle(), c))
        .collect();
    for r in &reports {
        assert!(r.ok(), "{r:?}");
        assert!(r.loads.passed() && r.plays.passed() && r.settings.passed());
    }
    let routed = reports.iter().find(|r| r.name == ROUTER_NAME).unwrap();
    assert!(routed.sidechain.passed() && routed.outputs.passed());
    let gain = reports.iter().find(|r| r.name != ROUTER_NAME).unwrap();
    assert!(matches!(gain.sidechain, Outcome::Skipped(_)));

    // A plugin that crashes on its first block is noted, not fatal.
    let crash = broken("DawTestCrashProcess", "crash-on-process");
    let class = (scan::scan_here(&crash).unwrap().into_iter())
        .find(|c| c.cid == PROCESSOR_CID)
        .unwrap();
    let r = check::check(helper(), &crash, &class);
    assert!(r.loads.passed());
    assert!(
        matches!(&r.plays, Outcome::Fail(why) if why.contains("crashed")),
        "{r:?}"
    );
    assert!(!r.ok());

    let md = check::markdown(&reports, "0.0.0", "test");
    assert!(md.contains("| DAW Test Router | DAW project (test fixture) | yes"));
    assert_eq!(md.lines().filter(|l| l.starts_with("| DAW")).count(), 2);
}
