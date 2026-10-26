// SPDX-License-Identifier: GPL-3.0-or-later
//! Spike 0.1 B, step one: list the VST 3 plugins installed on this machine.
//!
//! Only reads files. No plugin code is loaded or run. For each `.vst3`
//! found it reports the plugin's own `moduleinfo.json` (name, vendor,
//! version, categories) when the bundle has one, and which CPU
//! architectures it ships.
//!
//! Usage: `vst3-scan [extra folders...]`. The report is printed and also
//! saved as `vst3-scan-report.txt` in the current folder.

use std::fmt::Write as _;
use std::fs;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};

fn default_folders() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if cfg!(windows) {
        // The standard system-wide VST 3 location on Windows.
        if let Some(common) = std::env::var_os("CommonProgramFiles") {
            v.push(PathBuf::from(common).join("VST3"));
        } else {
            v.push(PathBuf::from(r"C:\Program Files\Common Files\VST3"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            v.push(
                PathBuf::from(local)
                    .join("Programs")
                    .join("Common")
                    .join("VST3"),
            );
        }
    } else if cfg!(target_os = "macos") {
        v.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
        if let Some(home) = std::env::var_os("HOME") {
            v.push(PathBuf::from(home).join("Library/Audio/Plug-Ins/VST3"));
        }
    } else {
        v.push(PathBuf::from("/usr/lib/vst3"));
        v.push(PathBuf::from("/usr/local/lib/vst3"));
        if let Some(home) = std::env::var_os("HOME") {
            v.push(PathBuf::from(home).join(".vst3"));
        }
    }
    v
}

/// Every `*.vst3` under `dir`, without descending into a bundle once found.
fn find_vst3(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        let is_vst3 = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("vst3"));
        if is_vst3 {
            found.push(path);
        } else if path.is_dir() {
            find_vst3(&path, found);
        }
    }
}

struct Report {
    path: PathBuf,
    kind: &'static str,
    arches: Vec<String>,
    name: Option<String>,
    vendor: Option<String>,
    version: Option<String>,
    classes: Vec<String>,
    note: Option<String>,
}

fn inspect(path: &Path) -> Report {
    let mut r = Report {
        path: path.to_owned(),
        kind: if path.is_dir() {
            "bundle"
        } else {
            "single file (older layout)"
        },
        arches: Vec::new(),
        name: None,
        vendor: None,
        version: None,
        classes: Vec::new(),
        note: None,
    };
    if !path.is_dir() {
        return r;
    }
    if let Ok(entries) = fs::read_dir(path.join("Contents")) {
        for e in entries.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n != "Resources" && e.path().is_dir() {
                r.arches.push(n);
            }
        }
        r.arches.sort();
    }
    let info = path
        .join("Contents")
        .join("Resources")
        .join("moduleinfo.json");
    match fs::read_to_string(&info) {
        Err(_) => r.note = Some("no moduleinfo.json (details need loading the plugin)".into()),
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Err(e) => r.note = Some(format!("moduleinfo.json not plain JSON: {e}")),
            Ok(v) => {
                let s = |v: &serde_json::Value, k: &str| {
                    v.get(k).and_then(|x| x.as_str()).map(str::to_owned)
                };
                r.name = s(&v, "Name");
                r.version = s(&v, "Version");
                r.vendor = v.get("Factory Info").and_then(|f| s(f, "Vendor"));
                if let Some(classes) = v.get("Classes").and_then(|c| c.as_array()) {
                    for c in classes {
                        // Audio processors only; controllers and other
                        // helper classes are noise here.
                        if s(c, "Category").as_deref() != Some("Audio Module Class") {
                            continue;
                        }
                        let subs = c
                            .get("Sub Categories")
                            .and_then(|x| x.as_array())
                            .map(|a| {
                                a.iter()
                                    .filter_map(|x| x.as_str())
                                    .collect::<Vec<_>>()
                                    .join("|")
                            })
                            .unwrap_or_default();
                        r.classes.push(format!(
                            "{} [{}]",
                            s(c, "Name").unwrap_or_else(|| "?".into()),
                            subs
                        ));
                    }
                }
            }
        },
    }
    r
}

fn main() {
    let mut folders = default_folders();
    folders.extend(std::env::args_os().skip(1).map(PathBuf::from));

    let mut out = String::new();
    let _ = writeln!(
        out,
        "VST 3 scan ({} {})",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let mut found = Vec::new();
    for f in &folders {
        let before = found.len();
        find_vst3(f, &mut found);
        let status = if f.is_dir() {
            format!("{} found", found.len() - before)
        } else {
            "not present".into()
        };
        let _ = writeln!(out, "  folder {}: {status}", f.display());
    }
    let _ = writeln!(out, "\n{} plugin file(s)\n", found.len());

    for (i, p) in found.iter().enumerate() {
        let r = inspect(p);
        let _ = writeln!(out, "{:>3}. {}", i + 1, r.path.display());
        let _ = writeln!(out, "     layout: {}", r.kind);
        if !r.arches.is_empty() {
            let _ = writeln!(out, "     binaries: {}", r.arches.join(", "));
        }
        if let Some(n) = &r.name {
            let _ = writeln!(out, "     name: {n}");
        }
        if let Some(v) = &r.vendor {
            let _ = writeln!(out, "     vendor: {v}");
        }
        if let Some(v) = &r.version {
            let _ = writeln!(out, "     version: {v}");
        }
        for c in &r.classes {
            let _ = writeln!(out, "     processor: {c}");
        }
        if let Some(n) = &r.note {
            let _ = writeln!(out, "     note: {n}");
        }
    }

    print!("{out}");
    let report = Path::new("vst3-scan-report.txt");
    match fs::write(report, &out) {
        Ok(()) => println!(
            "\nSaved to {}",
            fs::canonicalize(report).unwrap_or(report.into()).display()
        ),
        Err(e) => println!("\nCould not save the report: {e}"),
    }

    // Keep the window open when started by double-click on Windows.
    if cfg!(windows) {
        println!("Press Enter to close.");
        let _ = io::stdin().lock().lines().next();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_bundle_with_moduleinfo() {
        let root = std::env::temp_dir().join(format!("vst3-scan-test-{}", std::process::id()));
        let bundle = root.join("Vendor").join("Test Amp.vst3");
        fs::create_dir_all(bundle.join("Contents/x86_64-win")).unwrap();
        fs::create_dir_all(bundle.join("Contents/Resources")).unwrap();
        fs::write(
            bundle.join("Contents/Resources/moduleinfo.json"),
            r#"{"Name":"Test Amp","Version":"1.2.3","Factory Info":{"Vendor":"Example"},
               "Classes":[{"Category":"Audio Module Class","Name":"Test Amp",
                           "Sub Categories":["Fx","Distortion"]},
                          {"Category":"Component Controller Class","Name":"ctl"}]}"#,
        )
        .unwrap();
        fs::write(root.join("readme.txt"), "not a plugin").unwrap();

        let mut found = Vec::new();
        find_vst3(&root, &mut found);
        assert_eq!(found, vec![bundle.clone()]);

        let r = inspect(&bundle);
        assert_eq!(r.name.as_deref(), Some("Test Amp"));
        assert_eq!(r.vendor.as_deref(), Some("Example"));
        assert_eq!(r.arches, vec!["x86_64-win"]);
        assert_eq!(r.classes, vec!["Test Amp [Fx|Distortion]"]);
        fs::remove_dir_all(&root).unwrap();
    }
}
