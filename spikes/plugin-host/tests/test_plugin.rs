// SPDX-License-Identifier: GPL-3.0-or-later
//! Hosts our own test plugin end to end: load, process, automate, latency,
//! state. Runs on every CI platform without third-party plugins.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use plugin_host::checks::{self, BLOCK, RATE};
use plugin_host::com::ParamChanges;
use plugin_host::plugin::{Instance, Module};
use vst3::ComWrapper;
use vst3_test_plugin::{LATENCY, NAME, PROCESSOR_CID};

/// The test plugin laid out as a `.vst3` bundle, built once per test run.
fn bundle() -> PathBuf {
    static BUNDLE: OnceLock<PathBuf> = OnceLock::new();
    BUNDLE.get_or_init(build_bundle).clone()
}

fn build_bundle() -> PathBuf {
    let release = !cfg!(debug_assertions);
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(["build", "-p", "vst3-test-plugin"]);
    if release {
        cmd.arg("--release");
    }
    // A separate target dir avoids waiting on the lock held by `cargo test`.
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/test-plugin");
    cmd.arg("--target-dir").arg(&target);
    assert!(
        cmd.status().expect("run cargo").success(),
        "building the test plugin failed"
    );

    let profile = if release { "release" } else { "debug" };
    let (lib, sub, file) = if cfg!(windows) {
        (
            "vst3_test_plugin.dll",
            "x86_64-win",
            "DawSpikeTestGain.vst3",
        )
    } else if cfg!(target_os = "macos") {
        ("libvst3_test_plugin.dylib", "MacOS", "DawSpikeTestGain")
    } else {
        (
            "libvst3_test_plugin.so",
            "x86_64-linux",
            "DawSpikeTestGain.so",
        )
    };
    let root = target.join("DawSpikeTestGain.vst3");
    let dir = root.join("Contents").join(sub);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(target.join(profile).join(lib), dir.join(file)).unwrap();
    root
}

#[test]
fn full_check_passes() {
    let r = checks::check(&bundle(), false);
    println!("{}", r.render());
    assert!(r.passed(), "{}", r.render());
    assert_eq!(r.name, NAME);
    assert_eq!(r.kind, "effect");
    assert_eq!(r.latency, LATENCY);
    assert_eq!(r.params, 1);
    assert_eq!(r.state_bytes, 8);
    assert_eq!(r.state_restores_identically, Some(true));
    assert!(r.automation.is_some());
    assert_eq!(r.editor, "none");
}

#[test]
fn audio_automation_and_state_are_exact() {
    let module = Module::load(&bundle()).unwrap();
    let inst = Instance::new(&module, &PROCESSOR_CID, RATE, BLOCK).unwrap();

    let ramp: Vec<f32> = (0..BLOCK).map(|i| i as f32 / BLOCK as f32).collect();
    let input = vec![ramp.clone(), ramp.clone()];

    // Unity gain: the output is the input delayed by the reported latency.
    let first = inst.process(&input, None).unwrap();
    let l = LATENCY as usize;
    assert!(first[0][..l].iter().all(|&x| x == 0.0));
    assert_eq!(&first[0][l..], &ramp[..BLOCK - l]);

    // Automation to 0.5 applies from this block on.
    let half = ComWrapper::new(ParamChanges::with_point(0, 0, 0.5));
    let second = inst.process(&input, Some(&half)).unwrap();
    assert_eq!(&second[1][..l], &ramp[BLOCK - l..]); // tail of block 1, unity
    let expect: Vec<f32> = ramp[..BLOCK - l].iter().map(|x| x * 0.5).collect();
    assert_eq!(&second[1][l..], &expect[..]);

    // State carries the gain to a new instance.
    let (state, _) = inst.save_state().unwrap();
    assert_eq!(state, 0.5f64.to_le_bytes());
    let other = Instance::new(&module, &PROCESSOR_CID, RATE, BLOCK).unwrap();
    other.load_state(&state, None).unwrap();
    let out = other.process(&input, None).unwrap();
    assert_eq!(&out[0][l..], &expect[..]);
}
