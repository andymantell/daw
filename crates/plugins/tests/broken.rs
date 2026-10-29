// SPDX-License-Identifier: GPL-3.0-or-later
//! The plugin host against broken plugins (roadmap 1.9): builds of the
//! test plugin that crash or hang while loading, refuse to set up, or are
//! handed damaged state. None of them may take the host down. Also checks
//! that an effect's settings come back from a saved project.

#[path = "common/bundle.rs"]
mod bundle;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bundle::{broken, bundle};
use daw_engine::graph::Insert;
use daw_plugins::scan::{self, PluginList};
use daw_plugins::{Module, PluginState, instantiate};
use daw_project::{Effect, EffectState, Project, ProjectDoc, Track};
use daw_test_plugin::{LATENCY, NAME, PROCESSOR_CID};

const RATE: f64 = 48_000.0;
const BLOCK: usize = 256;

fn scanner() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_daw-plugin-helper"))
}

/// Runs a block of ones through `fx` and returns the left channel after
/// the plugin's latency.
fn ones_through(fx: &mut daw_plugins::LocalProcessor) -> Vec<f32> {
    let (mut a, mut b) = (vec![1.0; BLOCK], vec![1.0; BLOCK]);
    fx.process(&mut a, &mut b);
    a.split_off(LATENCY as usize)
}

#[test]
fn a_plugin_that_crashes_while_loading_is_reported() {
    let crash = broken("DawTestCrash", "crash-on-load");
    let err = scan::scan_in_child(scanner(), &crash, Duration::from_secs(60)).unwrap_err();
    assert!(err.contains("crashed while loading"), "{err}");

    // A rescan over a working plugin and the broken one keeps the working
    // one and lists the failure.
    let dirs = vec![
        bundle().parent().unwrap().to_owned(),
        crash.parent().unwrap().to_owned(),
    ];
    let mut list = PluginList::default();
    list.rescan(
        &dirs,
        |p| scan::scan_in_child(scanner(), p, Duration::from_secs(60)),
        |_| {},
    );
    assert_eq!(list.plugins()[0].class.name, NAME);
    let failures: Vec<_> = list.failures().collect();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].0, crash);
}

#[test]
fn a_plugin_that_hangs_while_loading_is_given_up_on() {
    let hang = broken("DawTestHang", "hang-on-load");
    let start = Instant::now();
    let err = scan::scan_in_child(scanner(), &hang, Duration::from_secs(2)).unwrap_err();
    assert!(err.contains("took more than 2 s"), "{err}");
    assert!(start.elapsed() < Duration::from_secs(20));
}

#[test]
fn a_plugin_that_refuses_to_set_up_is_an_error() {
    let refuse = broken("DawTestRefuse", "refuse-setup");
    let module = Arc::new(Module::load(&refuse).unwrap());
    let err = instantiate(module, &PROCESSOR_CID, RATE, BLOCK)
        .err()
        .expect("instantiating should fail");
    assert!(err.contains("setupProcessing"), "{err}");
}

#[test]
fn damaged_state_is_reported_and_the_plugin_carries_on() {
    let module = Arc::new(Module::load(&bundle()).unwrap());
    let (plugin, mut fx) = instantiate(module, &PROCESSOR_CID, RATE, BLOCK).unwrap();
    let id = plugin.params()[0].id;
    plugin.set_param(id, 0.5);
    for damaged in [vec![], vec![1, 2, 3]] {
        let state = PluginState {
            component: damaged,
            controller: None,
        };
        assert!(plugin.load_state(&state).is_err());
    }
    // Still running, with the setting it had.
    assert_eq!(plugin.param(id), Some(0.5));
    assert!(ones_through(&mut fx).iter().all(|&x| x == 0.5));
}

#[test]
fn an_effects_settings_come_back_from_a_saved_project() {
    let b = bundle();
    let module = Arc::new(Module::load(&b).unwrap());
    let (plugin, mut fx) = instantiate(module.clone(), &PROCESSOR_CID, RATE, BLOCK).unwrap();
    let id = plugin.params()[0].id;
    plugin.set_param(id, 0.25);
    // The change reaches the processor, whose state is what is saved,
    // with the next block.
    ones_through(&mut fx);

    // Saved as the app saves an effect.
    let saved = plugin.save_state().unwrap();
    let mut track = Track::new("t1".into(), "Guitar".into());
    track.effects.push(Effect {
        id: "e1".into(),
        name: NAME.into(),
        class: PROCESSOR_CID.iter().map(|&c| c as u8).collect(),
        bundle: b.display().to_string(),
        bypass: false,
        state: Some(EffectState {
            component: saved.component.clone(),
            controller: saved.controller.clone(),
        }),
        ..Default::default()
    });
    let project = Project {
        tracks: vec![track],
        ..Project::default()
    };
    let mut doc = ProjectDoc::new(1);
    doc.set(&project);
    let bytes = doc.to_bytes();
    drop(plugin);

    // Reopened by someone else, into a new instance.
    let back = ProjectDoc::from_bytes(&bytes, 2).unwrap().get();
    let e = &back.tracks[0].effects[0];
    let class: Vec<u8> = PROCESSOR_CID.iter().map(|&c| c as u8).collect();
    assert_eq!(e.class, class);
    let state = e.state.as_ref().expect("the effect's state was saved");
    let (plugin, mut fx) = instantiate(module, &PROCESSOR_CID, RATE, BLOCK).unwrap();
    plugin
        .load_state(&PluginState {
            component: state.component.clone(),
            controller: state.controller.clone(),
        })
        .unwrap();
    assert_eq!(plugin.param(id), Some(0.25));
    assert!(ones_through(&mut fx).iter().all(|&x| x == 0.25));
}
