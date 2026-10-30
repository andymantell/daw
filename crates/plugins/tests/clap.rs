// SPDX-License-Identifier: GPL-3.0-or-later
//! Hosts the test plugin in the CLAP format end to end: scanning, audio,
//! delay, parameters both ways, state, restarts, a sidechain and an
//! extra output, and broken builds. Each runs in the app's own process
//! and in a helper process, as the app may.

#[path = "common/bundle.rs"]
mod bundle;

use std::path::Path;
use std::time::Duration;

use bundle::{broken_clap, clap};
use daw_engine::graph::Insert;
use daw_plugins::clap::cid_of;
use daw_plugins::host::Host;
use daw_plugins::scan::{self, PluginList};
use daw_plugins::{Layout, Library, Plugin, Processor};
use daw_test_plugin::{CLAP_ID, CLAP_NAME, GAIN_PARAM, LATENCY, LEVEL_PARAM, LOOKAHEAD_PARAM};

const RATE: f64 = 48_000.0;
const BLOCK: usize = 256;

fn helper() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_daw-plugin-helper"))
}

/// The two sides of the test plugin from `file`, in this process or in
/// a helper (for rendering, so each block comes straight back).
fn open(file: &Path, remote: bool, layout: &Layout) -> (Plugin, Processor, Option<Host>) {
    let cid = cid_of(CLAP_ID);
    if remote {
        let host = Host::spawn(helper()).unwrap();
        let (p, x) = host
            .instantiate_with(file, &cid, RATE, BLOCK, None, layout)
            .unwrap();
        (Plugin::Remote(p), Processor::Remote(x), Some(host))
    } else {
        let lib = Library::load(file).unwrap();
        let (p, x) = lib.instantiate(&cid, RATE, BLOCK, layout).unwrap();
        (p, x, None)
    }
}

/// One block of all `main` (and a sidechain of all `key`); the left
/// channel that comes out.
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
fn scanning_finds_the_plugin_in_a_child_process() {
    let file = clap();
    let dirs = vec![file.parent().unwrap().to_owned()];
    assert_eq!(scan::find_bundles(&dirs), vec![file.clone()]);
    let mut list = PluginList::default();
    list.rescan(
        &dirs,
        |p| scan::scan_in_child(helper(), p, Duration::from_secs(60)),
        |_| {},
    );
    let plugins = list.plugins();
    assert_eq!(
        plugins.len(),
        1,
        "{:?}",
        list.failures().collect::<Vec<_>>()
    );
    let p = &plugins[0];
    assert_eq!(p.class.name, CLAP_NAME);
    assert_eq!(p.class.cid, cid_of(CLAP_ID));
    assert_eq!(p.class.vendor, "DAW project");
    assert_eq!(p.class.sub_categories, "Fx|utility|stereo");
    assert!(!p.class.is_instrument());
    assert_eq!(p.format(), "CLAP");
    // The list survives saving.
    assert_eq!(PluginList::from_text(&list.to_text()), list);
}

#[test]
fn audio_comes_through_delayed_with_its_gain() {
    for remote in [false, true] {
        let (plugin, mut fx, _host) = open(&clap(), remote, &Layout::default());
        assert_eq!(plugin.latency(), LATENCY, "remote {remote}");
        let out = block(&mut fx, 0.5, None);
        let n = LATENCY as usize;
        assert!(out[..n].iter().all(|&x| x == 0.0));
        assert!(out[n..].iter().all(|&x| x == 0.5), "remote {remote}");

        let params = plugin.params();
        let names: Vec<_> = params.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(names, ["Gain", "Level", "Lookahead"]);
        assert!(params[0].automatable);
        assert_eq!(params[2].steps, 1);
        // The plugin's own range (0 to 2) is 0 to 1 to the app.
        assert_eq!(plugin.param(GAIN_PARAM), Some(0.5));
        plugin.set_param(GAIN_PARAM, 0.25);
        assert_eq!(plugin.param(GAIN_PARAM), Some(0.25), "remote {remote}");
        assert_eq!(
            plugin.param_text(GAIN_PARAM, 0.25).as_deref(),
            Some("0.50x")
        );
        assert_eq!(block(&mut fx, 0.5, None), vec![0.25; BLOCK]);
    }
}

#[test]
fn changes_the_plugin_makes_itself_come_back() {
    for remote in [false, true] {
        let (mut plugin, mut fx, _host) = open(&clap(), remote, &Layout::default());
        let _ = plugin.poll();
        block(&mut fx, 0.75, None);
        let status = plugin.poll();
        assert!(status.edited, "remote {remote}");
        assert_eq!(plugin.param(LEVEL_PARAM), Some(0.75), "remote {remote}");
        assert!(!plugin.poll().edited, "reported once");
    }
}

#[test]
fn settings_come_back_in_a_second_copy() {
    for remote in [false, true] {
        let (plugin, mut fx, _host) = open(&clap(), remote, &Layout::default());
        plugin.set_param(GAIN_PARAM, 0.75);
        block(&mut fx, 0.0, None);
        let state = plugin.save_state().unwrap();
        assert_eq!(&state.component[..8], &1.5f64.to_le_bytes());

        let (other, mut fx2, _host2) = open(&clap(), remote, &Layout::default());
        other.load_state(&state).unwrap();
        assert_eq!(other.param(GAIN_PARAM), Some(0.75), "remote {remote}");
        let out = block(&mut fx2, 0.5, None);
        assert_eq!(out[LATENCY as usize], 0.75);
        // Damaged state is refused, and the plugin carries on.
        let bad = daw_plugins::PluginState {
            component: vec![1, 2],
            controller: None,
        };
        assert!(other.load_state(&bad).is_err());
        assert_eq!(other.param(GAIN_PARAM), Some(0.75));
    }
}

#[test]
fn a_delay_changed_while_playing_is_reported_once_restarted() {
    for remote in [false, true] {
        let (mut plugin, mut fx, _host) = open(&clap(), remote, &Layout::default());
        block(&mut fx, 1.0, None);
        assert!(!plugin.poll().restart);
        plugin.set_param(LOOKAHEAD_PARAM, 1.0);
        // The plugin hears the change with the next block, and asks.
        block(&mut fx, 1.0, None);
        let status = plugin.poll();
        assert!(status.restart, "remote {remote}");
        assert!(!status.reload);
        assert_eq!(status.latency, 2 * LATENCY, "remote {remote}");
        // Restarted: the delay line starts empty.
        let out = block(&mut fx, 1.0, None);
        let n = 2 * LATENCY as usize;
        assert!(
            out[..n].iter().all(|&x| x == 0.0),
            "remote {remote}: {out:?}"
        );
        assert!(out[n..].iter().all(|&x| x == 1.0));
        assert!(!plugin.poll().restart);
    }
}

#[test]
fn the_sidechain_and_extra_output_a_layout_asks_for() {
    for remote in [false, true] {
        let (plugin, mut fx, _host) = open(&clap(), remote, &Layout::default());
        let buses = plugin.buses();
        assert_eq!(buses.sidechain(), Some(1));
        assert_eq!(buses.inputs[1].name, "Sidechain");
        assert_eq!(buses.extra_outputs().count(), 1);
        // Main buses only: the sidechain is not heard, even if offered.
        let n = LATENCY as usize;
        assert_eq!(block(&mut fx, 0.0, Some(0.25))[n..], vec![0.0; BLOCK - n]);

        let both = Layout {
            sidechain: true,
            outputs: vec![1],
        };
        let (_p, mut fx, _host) = open(&clap(), remote, &both);
        // The sidechain into the main output, and half of it again from
        // the extra output.
        assert_eq!(block(&mut fx, 0.0, Some(0.25)), vec![0.375; BLOCK]);
        assert_eq!(block(&mut fx, 0.0, None), vec![0.0; BLOCK]);
    }
}

#[test]
fn a_plugin_that_refuses_to_start_is_skipped() {
    let file = broken_clap("DawTestRefuse", "refuse-setup");
    let (_p, mut fx, _host) = open(&file, false, &Layout::default());
    assert_eq!(block(&mut fx, 0.5, None), vec![0.5; BLOCK]);
}

#[test]
fn a_plugin_that_crashes_while_playing_takes_only_its_helper_down() {
    let file = broken_clap("DawTestCrashProcess", "crash-on-process");
    let (plugin, mut fx, _host) = open(&file, true, &Layout::default());
    let _ = block(&mut fx, 0.5, None);
    assert!(plugin.failure().is_some());
}

#[test]
fn a_plugin_without_a_window_says_so() {
    let (mut plugin, _fx, _host) = open(&clap(), false, &Layout::default());
    let err = plugin.open_editor("Test").unwrap_err();
    assert!(err.contains("no editor"), "{err}");
    assert!(!plugin.editor_open());
}
