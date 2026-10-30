// SPDX-License-Identifier: GPL-3.0-or-later
//! Hosts the DAW test plugin end to end: scanning, audio, latency,
//! parameters, state, and the engine running it as a track insert.
//! Needs no third-party plugins, so it runs on every CI platform.

#[path = "common/bundle.rs"]
mod bundle;
#[path = "../../engine/tests/common/rt_check.rs"]
mod rt_check;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bundle::bundle;
use daw_engine::graph::{AudioData, ClipSpec, Insert, InsertSpec, Session, TrackSpec};
use daw_engine::time::{Frames, Tempo, TempoMap, Ticks, TimeSignature};
use daw_plugins::scan::{self, PluginList};
use daw_plugins::{Module, PluginState, instantiate};
use daw_test_plugin::{LATENCY, NAME, PROCESSOR_CID, ROUTER_NAME, SYNTH_NAME};

const RATE: u32 = 48_000;
const BLOCK: usize = 256;

fn ramp() -> Vec<f32> {
    (0..BLOCK).map(|i| (i + 1) as f32 / BLOCK as f32).collect()
}

#[test]
fn scanning_finds_both_classes_in_a_child_process() {
    let b = bundle();
    let dirs = vec![b.parent().unwrap().to_owned()];
    assert_eq!(scan::find_bundles(&dirs), vec![b.clone()]);

    let exe = Path::new(env!("CARGO_BIN_EXE_daw-plugin-helper"));
    let mut list = PluginList::default();
    let mut seen = Vec::new();
    list.rescan(
        &dirs,
        |p| scan::scan_in_child(exe, p, Duration::from_secs(60)),
        |p| seen.push(p.to_owned()),
    );
    assert_eq!(seen, vec![b.clone()]);
    let plugins = list.plugins();
    let names: Vec<_> = plugins.iter().map(|p| p.class.name.as_str()).collect();
    // Effects first, then instruments.
    assert_eq!(names, vec![NAME, ROUTER_NAME, SYNTH_NAME]);
    assert!(plugins[2].class.is_instrument());
    assert_eq!(plugins[0].class.cid, PROCESSOR_CID);

    // Unchanged bundles are not scanned again.
    list.rescan(&dirs, |_| panic!("rescanned"), |_| {});

    // The list survives saving, including what the user turned off.
    list.set_enabled(&PROCESSOR_CID, false);
    let back = PluginList::from_text(&list.to_text());
    assert_eq!(back, list);
    assert!(!back.plugins()[0].enabled);
}

#[test]
fn a_bundle_that_will_not_load_is_reported_not_fatal() {
    let dir = std::env::temp_dir().join(format!("daw-bad-plugin-{}", std::process::id()));
    let bad = dir.join("Broken.vst3");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&bad, b"not a plugin").unwrap();
    let exe = Path::new(env!("CARGO_BIN_EXE_daw-plugin-helper"));
    let err = scan::scan_in_child(exe, &bad, Duration::from_secs(60)).unwrap_err();
    assert!(err.contains("could not load"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn audio_latency_parameters_and_state() {
    let module = Arc::new(Module::load(&bundle()).unwrap());
    let (plugin, mut fx) = instantiate(module.clone(), &PROCESSOR_CID, RATE as f64, BLOCK).unwrap();
    assert_eq!(plugin.latency(), LATENCY);
    let params = plugin.params();
    assert_eq!(params.len(), 1);
    assert!(params[0].automatable);

    // Unity gain: the input, delayed by the reported latency.
    let l = LATENCY as usize;
    let (mut a, mut b) = (ramp(), ramp());
    fx.process(&mut a, &mut b);
    assert!(a[..l].iter().all(|&x| x == 0.0));
    assert_eq!(&a[l..], &ramp()[..BLOCK - l]);
    assert_eq!(a, b);

    // A parameter set from the UI reaches the processor with its next
    // block, and the controller knows it at once.
    plugin.set_param(params[0].id, 0.5);
    assert_eq!(plugin.param(params[0].id), Some(0.5));
    let (mut a, mut b) = (ramp(), ramp());
    fx.process(&mut a, &mut b);
    let half: Vec<f32> = ramp()[..BLOCK - l].iter().map(|x| x * 0.5).collect();
    assert_eq!(&a[l..], &half[..]);

    // State carries the gain to a new instance.
    let state = plugin.save_state().unwrap();
    assert_eq!(state.component, 0.5f64.to_le_bytes());
    let (other, mut fx2) = instantiate(module, &PROCESSOR_CID, RATE as f64, BLOCK).unwrap();
    other.load_state(&state).unwrap();
    assert_eq!(other.param(params[0].id), Some(0.5));
    let (mut a, mut b) = (ramp(), ramp());
    fx2.process(&mut a, &mut b);
    assert_eq!(&a[l..], &half[..]);
    assert_eq!(
        other.save_state().unwrap(),
        PluginState {
            component: 0.5f64.to_le_bytes().to_vec(),
            ..state
        }
    );
}

#[test]
fn blocks_longer_than_set_up_for_are_split() {
    let module = Arc::new(Module::load(&bundle()).unwrap());
    let (_plugin, mut fx) = instantiate(module, &PROCESSOR_CID, RATE as f64, 64).unwrap();
    let input: Vec<f32> = (0..1000).map(|i| i as f32).collect();
    let (mut a, mut b) = (input.clone(), input.clone());
    fx.process(&mut a, &mut b);
    let l = LATENCY as usize;
    assert_eq!(&a[l..], &input[..1000 - l]);
}

#[test]
fn the_engine_compensates_the_plugins_latency() {
    let module = Arc::new(Module::load(&bundle()).unwrap());
    let (plugin, fx) = instantiate(module, &PROCESSOR_CID, RATE as f64, BLOCK).unwrap();
    let clip = || {
        let data = Arc::new(AudioData {
            channels: vec![(0..4000).map(|i| (i + 1) as f32 / 1e6).collect()],
            rate: RATE,
        });
        ClipSpec::new(Ticks(0), data)
    };
    let mut s = Session::new(TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON));
    let mut dry = TrackSpec::new(vec![clip()]);
    dry.mix.pan = -1.0;
    let mut wet = TrackSpec::new(vec![clip()]);
    wet.mix.pan = 1.0;
    wet.inserts = vec![InsertSpec {
        id: 1,
        bypass: false,
        latency: plugin.latency(),
        sidechain: None,
        ..Default::default()
    }];
    s.tracks = vec![dry, wet];
    let mut fx: Vec<(u64, Box<dyn Insert>)> = vec![(1, Box::new(fx))];
    let g = s.compile_with(BLOCK, &mut fx).unwrap();
    let [l, r] = daw_engine::engine::render_offline(g, Frames(0), 3000, BLOCK);
    let lat = LATENCY as usize;
    for i in lat..3000 {
        assert!(
            (l[i] - r[i]).abs() < 1e-9,
            "frame {i}: {} vs {}",
            l[i],
            r[i]
        );
    }
}

#[test]
fn a_lane_sets_the_plugins_setting_block_by_block() {
    use daw_engine::automation::{Curve, LaneSpec, Target};

    let module = Arc::new(Module::load(&bundle()).unwrap());
    let (plugin, fx) = instantiate(module, &PROCESSOR_CID, RATE as f64, BLOCK).unwrap();
    let gain = plugin.params()[0].id;
    let data = Arc::new(AudioData {
        channels: vec![vec![1.0; 4000]],
        rate: RATE,
    });
    let mut s = Session::new(TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON));
    let mut t = TrackSpec::new(vec![ClipSpec::new(Ticks(0), data)]);
    t.inserts = vec![InsertSpec {
        id: 1,
        bypass: false,
        latency: plugin.latency(),
        sidechain: None,
        ..Default::default()
    }];
    // Half gain for the first 2048 frames, then a quarter.
    t.automation = vec![LaneSpec {
        target: Target::Insert { id: 1, param: gain },
        curve: Arc::new(Curve::new(vec![(0, 0.5), (2047, 0.5), (2048, 0.25)])),
        live: false,
    }];
    s.tracks = vec![t];
    let mut fx: Vec<(u64, Box<dyn Insert>)> = vec![(1, Box::new(fx))];
    let g = s.compile_with(BLOCK, &mut fx).unwrap();
    let [l, _] = daw_engine::engine::render_offline(g, Frames(0), 4000, BLOCK);
    // Mono at centre pan: the track's own law, read off a settled frame.
    let law = l[1000] / 0.5;
    assert!((l[1500] / law - 0.5).abs() < 1e-6, "{}", l[1500] / law);
    assert!((l[3000] / law - 0.25).abs() < 1e-6, "{}", l[3000] / law);
    // The controller was never told: the lane is the processor's alone.
    assert_eq!(plugin.param(gain), Some(1.0));
}

#[test]
fn processing_does_not_allocate() {
    let module = Arc::new(Module::load(&bundle()).unwrap());
    let (plugin, mut fx) = instantiate(module, &PROCESSOR_CID, RATE as f64, BLOCK).unwrap();
    let id = plugin.params()[0].id;
    let (mut a, mut b) = (ramp(), ramp());
    fx.process(&mut a, &mut b);
    for v in [0.25, 0.75, 1.0] {
        plugin.set_param(id, v);
        let before = rt_check::violations();
        {
            let _rt = rt_check::RtGuard::enter();
            fx.process(&mut a, &mut b);
        }
        assert_eq!(
            rt_check::violations(),
            before,
            "allocated with a parameter change"
        );
    }
}
