// SPDX-License-Identifier: GPL-3.0-or-later
//! Finding installed plugins, VST 3 and CLAP, and remembering what
//! they are.
//!
//! Loading a module runs its code, and a broken one can crash or hang
//! the process that loads it. So each bundle is opened in a child
//! process ([`scan_in_child`], which runs the app again with
//! [`SCAN_ARG`]); the app only reads what the child printed. Results are
//! kept in a [`PluginList`] saved as text, and a bundle is scanned again
//! only when its modification time changes.
//!
//! Scanning also notes which older classes each plugin says it stands
//! in for ([`crate::compat`]), so a project saved with an older version
//! opens with the newer one ([`PluginList::replacement_for`]).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use crate::host::Sandbox;
use crate::module::{ClassInfo, Module, Result, cid_from_hex, cid_to_hex};

/// The argument that makes the app scan one bundle and exit.
pub const SCAN_ARG: &str = "--scan-vst3";

/// How long a bundle may take to load before it is given up on.
pub const SCAN_TIMEOUT: Duration = Duration::from_secs(30);

/// The standard plugin folders for this platform: VST 3's, as its
/// "Plug-in Locations" documentation lists them, then CLAP's
/// ([`clap_dirs`]).
pub fn default_dirs() -> Vec<PathBuf> {
    let mut dirs = vst3_dirs();
    dirs.extend(clap_dirs());
    dirs
}

/// The standard CLAP folders for this platform, as the CLAP headers
/// (`entry.h`) list them, and those in the `CLAP_PATH` variable.
pub fn clap_dirs() -> Vec<PathBuf> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let mut dirs = Vec::new();
    if cfg!(windows) {
        if let Some(common) = env("CommonProgramFiles") {
            dirs.push(common.join("CLAP"));
        }
        if let Some(local) = env("LOCALAPPDATA") {
            dirs.push(local.join("Programs").join("Common").join("CLAP"));
        }
    } else if cfg!(target_os = "macos") {
        dirs.push("/Library/Audio/Plug-Ins/CLAP".into());
        if let Some(home) = env("HOME") {
            dirs.push(home.join("Library/Audio/Plug-Ins/CLAP"));
        }
    } else {
        if let Some(home) = env("HOME") {
            dirs.push(home.join(".clap"));
        }
        dirs.push("/usr/lib/clap".into());
    }
    if let Some(path) = std::env::var_os("CLAP_PATH") {
        dirs.extend(std::env::split_paths(&path).filter(|p| !p.as_os_str().is_empty()));
    }
    dirs
}

/// The standard VST 3 folders for this platform.
pub fn vst3_dirs() -> Vec<PathBuf> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let mut dirs = Vec::new();
    if cfg!(windows) {
        if let Some(local) = env("LOCALAPPDATA") {
            dirs.push(local.join("Programs").join("Common").join("VST3"));
        }
        if let Some(common) = env("CommonProgramFiles") {
            dirs.push(common.join("VST3"));
        }
    } else if cfg!(target_os = "macos") {
        if let Some(home) = env("HOME") {
            dirs.push(home.join("Library/Audio/Plug-Ins/VST3"));
        }
        dirs.push("/Library/Audio/Plug-Ins/VST3".into());
    } else {
        if let Some(home) = env("HOME") {
            dirs.push(home.join(".vst3"));
        }
        dirs.push("/usr/lib/vst3".into());
        dirs.push("/usr/local/lib/vst3".into());
    }
    dirs
}

/// Every `.vst3` and `.clap` bundle (or single file) under `dirs`,
/// sorted.
pub fn find_bundles(dirs: &[PathBuf]) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let path = e.path();
            let is_plugin = path
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("vst3") || x.eq_ignore_ascii_case("clap"));
            if is_plugin {
                out.push(path);
            } else if depth < 8 && path.is_dir() {
                walk(&path, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    for d in dirs {
        walk(d, 0, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

/// Loads `bundle` in this process and lists its classes, with what
/// each replaces.
pub fn scan_here(bundle: &Path) -> Result<Vec<ClassInfo>> {
    if crate::clap::is_clap(bundle) {
        return Ok(crate::clap::ClapModule::load(bundle)?.classes());
    }
    let module = Module::load(bundle)?;
    let replaces = crate::compat::of_module(&module);
    let mut classes = module.classes();
    for c in &mut classes {
        c.replaces = (replaces.iter())
            .filter(|r| r.new == c.cid)
            .map(|r| r.old)
            .collect();
    }
    Ok(classes)
}

fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

fn class_line(c: &ClassInfo) -> String {
    let mut line = format!(
        "class\t{}\t{}\t{}\t{}\t{}",
        cid_to_hex(&c.cid),
        clean(&c.name),
        clean(&c.category),
        clean(&c.sub_categories),
        clean(&c.vendor)
    );
    if !c.replaces.is_empty() {
        let old: Vec<String> = c.replaces.iter().map(cid_to_hex).collect();
        let _ = write!(line, "\t{}", old.join(","));
    }
    line
}

/// A class line's fields; lists written before classes could replace
/// others have one field fewer.
fn parse_class(fields: &[&str]) -> Option<ClassInfo> {
    let (cid, name, category, sub, vendor, replaces) = match fields {
        [cid, name, category, sub, vendor] => (cid, name, category, sub, vendor, ""),
        [cid, name, category, sub, vendor, replaces] => {
            (cid, name, category, sub, vendor, *replaces)
        }
        _ => return None,
    };
    Some(ClassInfo {
        cid: cid_from_hex(cid)?,
        name: name.to_string(),
        category: category.to_string(),
        sub_categories: sub.to_string(),
        vendor: vendor.to_string(),
        replaces: (replaces.split(','))
            .filter(|s| !s.is_empty())
            .map(cid_from_hex)
            .collect::<Option<_>>()?,
    })
}

/// The child's side: scans `bundle`, prints the result, and returns the
/// exit code.
pub fn child_main(bundle: &Path) -> i32 {
    match scan_here(bundle) {
        Ok(classes) => {
            for c in &classes {
                println!("{}", class_line(c));
            }
            0
        }
        Err(e) => {
            println!("error\t{}", clean(&e));
            1
        }
    }
}

/// Scans `bundle` by running `exe SCAN_ARG bundle` and reading its
/// output. A crash, a hang past `timeout` or garbled output is an error.
pub fn scan_in_child(exe: &Path, bundle: &Path, timeout: Duration) -> Result<Vec<ClassInfo>> {
    let mut child = Command::new(exe)
        .arg(SCAN_ARG)
        .arg(bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start the scanner: {e}"))?;
    // Read on a thread so a chatty child cannot block on a full pipe.
    let mut stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut s);
        s
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "the plugin took more than {} s to load",
                    timeout.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(format!("lost the scanner: {e}")),
        }
    };
    let out = reader.join().unwrap_or_default();
    let mut classes = Vec::new();
    for line in out.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.first() {
            Some(&"class") => classes.push(parse_class(&fields[1..]).ok_or("garbled scan output")?),
            Some(&"error") => return Err(fields.get(1).unwrap_or(&"").to_string()),
            _ => {}
        }
    }
    if !status.success() {
        return Err(match status.code() {
            Some(c) => format!("the plugin crashed while loading (exit code {c})"),
            None => "the plugin crashed while loading".into(),
        });
    }
    Ok(classes)
}

/// What scanning one bundle found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleEntry {
    pub path: PathBuf,
    /// Modification time when scanned, in seconds since 1970.
    pub modified: u64,
    pub result: std::result::Result<Vec<ClassInfo>, String>,
}

fn modified(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

/// A plugin the user can pick: one audio class of a bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Available {
    pub bundle: PathBuf,
    pub class: ClassInfo,
    pub enabled: bool,
    /// Where the user wants it to run.
    pub sandbox: Sandbox,
}

impl Available {
    /// The plugin's format, for showing: "CLAP" or "VST 3".
    pub fn format(&self) -> &'static str {
        format_of(&self.bundle)
    }
}

/// The format of the plugin file at `path`, for showing.
pub fn format_of(path: &Path) -> &'static str {
    if crate::clap::is_clap(path) {
        "CLAP"
    } else {
        "VST 3"
    }
}

/// Every scanned bundle, and which plugins the user has turned off.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginList {
    pub bundles: Vec<BundleEntry>,
    /// Class IDs (hex) the user disabled.
    pub disabled: BTreeSet<String>,
    /// Where each plugin runs, by class ID (hex), where the user chose.
    pub sandbox: BTreeMap<String, Sandbox>,
    /// Most plugin windows open at once (0 for no limit): opening one
    /// more closes the one opened longest ago, to spare the computer.
    pub editors: u32,
}

impl PluginList {
    /// Scans bundles under `dirs` that are new or changed, using `scan`
    /// for each (normally [`scan_in_child`]), and forgets bundles that
    /// are gone. `progress` hears each bundle before it is scanned.
    pub fn rescan(
        &mut self,
        dirs: &[PathBuf],
        mut scan: impl FnMut(&Path) -> Result<Vec<ClassInfo>>,
        mut progress: impl FnMut(&Path),
    ) {
        let found = find_bundles(dirs);
        self.bundles.retain(|b| found.contains(&b.path));
        for path in found {
            let m = modified(&path);
            let known = self.bundles.iter().position(|b| b.path == path);
            if let Some(i) = known
                && self.bundles[i].modified == m
            {
                continue;
            }
            progress(&path);
            let entry = BundleEntry {
                result: scan(&path),
                path,
                modified: m,
            };
            match known {
                Some(i) => self.bundles[i] = entry,
                None => self.bundles.push(entry),
            }
        }
        self.bundles.sort_by(|a, b| a.path.cmp(&b.path));
    }

    /// Effects first, then instruments, each by name.
    pub fn plugins(&self) -> Vec<Available> {
        let mut out: Vec<Available> = self
            .bundles
            .iter()
            .filter_map(|b| Some((b, b.result.as_ref().ok()?)))
            .flat_map(|(b, classes)| {
                classes
                    .iter()
                    .filter(|c| c.is_audio_plugin())
                    .map(|c| Available {
                        bundle: b.path.clone(),
                        enabled: !self.disabled.contains(&cid_to_hex(&c.cid)),
                        sandbox: self.sandbox_of(&c.cid),
                        class: c.clone(),
                    })
            })
            .collect();
        out.sort_by_key(|a| (a.class.is_instrument(), a.class.name.to_lowercase()));
        out
    }

    /// The installed plugin that stands in for class `old` (an older
    /// version of it, by its own declaration), if there is one and it is
    /// switched on. A plugin installed as itself is not replaced.
    pub fn replacement_for(&self, old: &vst3::Steinberg::TUID) -> Option<Available> {
        let plugins = self.plugins();
        if plugins.iter().any(|a| a.class.cid == *old) {
            return None;
        }
        plugins
            .into_iter()
            .find(|a| a.enabled && a.class.replaces.contains(old))
    }

    /// Bundles that failed to scan, with why.
    pub fn failures(&self) -> impl Iterator<Item = (&Path, &str)> {
        self.bundles
            .iter()
            .filter_map(|b| Some((b.path.as_path(), b.result.as_ref().err()?.as_str())))
    }

    pub fn set_enabled(&mut self, cid: &vst3::Steinberg::TUID, enabled: bool) {
        let hex = cid_to_hex(cid);
        if enabled {
            self.disabled.remove(&hex);
        } else {
            self.disabled.insert(hex);
        }
    }

    /// Where a plugin runs: the user's choice, or the default.
    pub fn sandbox_of(&self, cid: &vst3::Steinberg::TUID) -> Sandbox {
        self.sandbox
            .get(&cid_to_hex(cid))
            .copied()
            .unwrap_or_default()
    }

    pub fn set_sandbox(&mut self, cid: &vst3::Steinberg::TUID, sandbox: Sandbox) {
        self.sandbox.insert(cid_to_hex(cid), sandbox);
    }

    /// Forgets a bundle's result so the next rescan tries it again.
    pub fn forget(&mut self, bundle: &Path) {
        self.bundles.retain(|b| b.path != bundle);
    }

    pub fn to_text(&self) -> String {
        let mut s = String::from("# DAW plugin list. Rebuilt by rescanning; safe to delete.\n");
        for b in &self.bundles {
            let _ = writeln!(s, "bundle\t{}\t{}", b.modified, b.path.display());
            match &b.result {
                Ok(classes) => {
                    for c in classes {
                        let _ = writeln!(s, "{}", class_line(c));
                    }
                }
                Err(e) => {
                    let _ = writeln!(s, "error\t{}", clean(e));
                }
            }
        }
        for d in &self.disabled {
            let _ = writeln!(s, "disabled\t{d}");
        }
        for (cid, sb) in &self.sandbox {
            let _ = writeln!(s, "sandbox\t{cid}\t{}", sb.key());
        }
        if self.editors > 0 {
            let _ = writeln!(s, "editors\t{}", self.editors);
        }
        s
    }

    /// Reads [`to_text`](Self::to_text) output; lines it does not
    /// understand are skipped.
    pub fn from_text(text: &str) -> PluginList {
        let mut list = PluginList::default();
        for line in text.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            match fields.as_slice() {
                ["bundle", m, path] => list.bundles.push(BundleEntry {
                    path: PathBuf::from(path),
                    modified: m.parse().unwrap_or(0),
                    result: Ok(Vec::new()),
                }),
                ["class", rest @ ..] => {
                    if let (Some(b), Some(c)) = (list.bundles.last_mut(), parse_class(rest))
                        && let Ok(classes) = &mut b.result
                    {
                        classes.push(c);
                    }
                }
                ["error", e] => {
                    if let Some(b) = list.bundles.last_mut() {
                        b.result = Err(e.to_string());
                    }
                }
                ["disabled", d] => {
                    list.disabled.insert(d.to_string());
                }
                ["sandbox", cid, key] => {
                    if let Some(sb) = Sandbox::from_key(key) {
                        list.sandbox.insert(cid.to_string(), sb);
                    }
                }
                ["editors", n] => list.editors = n.parse().unwrap_or(0),
                _ => {}
            }
        }
        list
    }
}
