// SPDX-License-Identifier: GPL-3.0-or-later
//! The DAW test plugin laid out as `.vst3` bundles for the host's tests:
//! the working one, and broken builds of it (see its Cargo features).
//! The same library is laid out as a `.clap` plugin too.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

/// The working test plugin, in a folder of its own.
#[allow(dead_code)]
pub fn bundle() -> PathBuf {
    build("DawTestGain", None)
}

/// The working test plugin as a CLAP plugin, in a folder of its own: a
/// `.clap` file, or a bundle on macOS.
#[allow(dead_code)]
pub fn clap() -> PathBuf {
    clap_named("DawTestGain", None)
}

/// A broken build of the test plugin as a CLAP plugin.
#[allow(dead_code)]
pub fn broken_clap(name: &'static str, feature: &'static str) -> PathBuf {
    clap_named(name, Some(feature))
}

/// As [`build`]: once per test process for each name, by a rename.
fn clap_named(name: &'static str, feature: Option<&'static str>) -> PathBuf {
    static MADE: Mutex<Option<HashMap<&'static str, PathBuf>>> = Mutex::new(None);
    let mut made = MADE.lock().unwrap_or_else(|e| e.into_inner());
    let made = made.get_or_insert_with(HashMap::new);
    if let Some(p) = made.get(name) {
        return p.clone();
    }
    let vst3 = build(name, feature);
    let lib = std::fs::read_dir(binary_dir(&vst3))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_none_or(|x| x != "part"))
        .unwrap();
    let root = vst3
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(format!("clap-{name}"))
        .join(format!("{name}.clap"));
    let to = if cfg!(target_os = "macos") {
        root.join("Contents").join("MacOS").join(name)
    } else {
        root.clone()
    };
    std::fs::create_dir_all(to.parent().unwrap()).unwrap();
    let new = std::fs::read(&lib).unwrap();
    if std::fs::read(&to).ok().as_deref() != Some(new.as_slice()) {
        let part = to.with_extension(format!("{}.part", std::process::id()));
        std::fs::write(&part, &new).unwrap();
        std::fs::rename(&part, &to).unwrap();
    }
    made.insert(name, root.clone());
    root
}

/// The folder inside a `.vst3` bundle that holds its binary.
fn binary_dir(bundle: &Path) -> PathBuf {
    let sub = if cfg!(windows) {
        "x86_64-win".to_owned()
    } else if cfg!(target_os = "macos") {
        "MacOS".to_owned()
    } else {
        format!("{}-linux", std::env::consts::ARCH)
    };
    bundle.join("Contents").join(sub)
}

/// A broken build of the test plugin, made with Cargo feature `feature`
/// and named `name`, in a folder of its own.
#[allow(dead_code)]
pub fn broken(name: &'static str, feature: &'static str) -> PathBuf {
    build(name, Some(feature))
}

/// The plugin helper binary, for tests outside the plugins crate (which
/// has it as `CARGO_BIN_EXE_daw-plugin-helper`). Built once per test
/// process, in a target dir of its own; Cargo's own lock keeps processes
/// building it side by side in step.
#[allow(dead_code)]
pub fn helper() -> PathBuf {
    static BUILT: Mutex<Option<PathBuf>> = Mutex::new(None);
    let mut built = BUILT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(p) = &*built {
        return p.clone();
    }
    let release = !cfg!(debug_assertions);
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(["build", "-p", "daw-plugins", "--bin", "daw-plugin-helper"]);
    if release {
        cmd.arg("--release");
    }
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-plugin/helper");
    cmd.arg("--target-dir").arg(&target);
    assert!(
        cmd.status().expect("run cargo").success(),
        "building the plugin helper failed"
    );
    let profile = if release { "release" } else { "debug" };
    let exe = target
        .join(profile)
        .join(format!("daw-plugin-helper{}", std::env::consts::EXE_SUFFIX));
    *built = Some(exe.clone());
    exe
}

/// Builds once per test process for each name. Within a process the
/// builds take turns. Test runners that start a process per test (such as
/// nextest) build side by side, so each name has its own target dir, and
/// the bundle is only replaced when its contents change, by a rename, so
/// no process loads a half-written or overwritten library.
fn build(name: &'static str, feature: Option<&'static str>) -> PathBuf {
    static BUILT: Mutex<Option<HashMap<&'static str, PathBuf>>> = Mutex::new(None);
    let mut built = BUILT.lock().unwrap_or_else(|e| e.into_inner());
    let built = built.get_or_insert_with(HashMap::new);
    if let Some(p) = built.get(name) {
        return p.clone();
    }
    let release = !cfg!(debug_assertions);
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(["build", "-p", "daw-test-plugin"]);
    if release {
        cmd.arg("--release");
    }
    if let Some(f) = feature {
        cmd.args(["--features", f]);
    }
    // A separate target dir avoids waiting on the lock held by `cargo test`.
    let target = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-plugin")
        .join(name);
    cmd.arg("--target-dir").arg(&target);
    assert!(
        cmd.status().expect("run cargo").success(),
        "building the test plugin failed"
    );
    let profile = if release { "release" } else { "debug" };
    let (lib, sub, file) = if cfg!(windows) {
        (
            "daw_test_plugin.dll",
            "x86_64-win".to_owned(),
            format!("{name}.vst3"),
        )
    } else if cfg!(target_os = "macos") {
        (
            "libdaw_test_plugin.dylib",
            "MacOS".to_owned(),
            name.to_owned(),
        )
    } else {
        (
            "libdaw_test_plugin.so",
            format!("{}-linux", std::env::consts::ARCH),
            format!("{name}.so"),
        )
    };
    let root = target
        .join(format!("plugins-{name}"))
        .join(format!("{name}.vst3"));
    let dir = root.join("Contents").join(sub);
    std::fs::create_dir_all(&dir).unwrap();
    let from = target.join(profile).join(lib);
    let to = dir.join(file);
    let new = std::fs::read(&from).unwrap();
    if std::fs::read(&to).ok().as_deref() != Some(new.as_slice()) {
        let part = dir.join(format!("{}.part", std::process::id()));
        std::fs::write(&part, &new).unwrap();
        std::fs::rename(&part, &to).unwrap();
    }
    built.insert(name, root.clone());
    root
}
