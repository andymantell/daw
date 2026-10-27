// SPDX-License-Identifier: GPL-3.0-or-later
//! Spike 0.1 B test runner.
//!
//!   plugin-host check <plugin.vst3> [--editor]
//!   plugin-host check-all [folder...] [--editor]
//!   plugin-host oop <plugin.vst3>... [--seconds 20] [--block 256] [--no-kill]
//!
//! `check-all` scans the standard VST 3 folders (plus any given) and checks
//! each plugin in its own child process, so a crashing or hanging plugin
//! can't take the run down. The summary is saved as
//! `plugin-host-report.txt`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use plugin_host::{checks, oop, oop_bench};

const CHILD_TIMEOUT: Duration = Duration::from_secs(120);

fn default_folders() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if cfg!(windows) {
        let common = std::env::var_os("CommonProgramFiles")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Program Files\Common Files"));
        v.push(common.join("VST3"));
    } else if cfg!(target_os = "macos") {
        v.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
    } else {
        v.push(PathBuf::from("/usr/lib/vst3"));
        if let Some(home) = std::env::var_os("HOME") {
            v.push(PathBuf::from(home).join(".vst3"));
        }
    }
    v
}

fn find_vst3(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        if p.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("vst3"))
        {
            out.push(p);
        } else if p.is_dir() {
            find_vst3(&p, out);
        }
    }
}

fn run_child(exe: &Path, plugin: &Path, editor: bool) -> (bool, String) {
    let mut cmd = Command::new(exe);
    cmd.arg("check")
        .arg(plugin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if editor {
        cmd.arg("--editor");
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (false, format!("could not start checker: {e}\n")),
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = child
                    .wait_with_output()
                    .map(|o| o.stdout)
                    .unwrap_or_default();
                let text = String::from_utf8_lossy(&out).into_owned();
                return if status.success() {
                    (true, text)
                } else if text.contains("result:") {
                    (false, text)
                } else {
                    (
                        false,
                        format!(
                            "plugin: {}\n  result: FAIL: checker crashed ({status})\n",
                            plugin.display()
                        ),
                    )
                };
            }
            Ok(None) if start.elapsed() > CHILD_TIMEOUT => {
                let _ = child.kill();
                return (
                    false,
                    format!(
                        "plugin: {}\n  result: FAIL: hung for {}s\n",
                        plugin.display(),
                        CHILD_TIMEOUT.as_secs()
                    ),
                );
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => {
                return (
                    false,
                    format!("plugin: {}\n  result: FAIL: {e}\n", plugin.display()),
                );
            }
        }
    }
}

fn pause() {
    if cfg!(windows) {
        println!("Press Enter to close.");
        let mut s = String::new();
        let _ = std::io::stdin().read_line(&mut s);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let editor = args.iter().any(|a| a == "--editor");
    let rest: Vec<&String> = args.iter().skip(1).filter(|a| *a != "--editor").collect();
    match args.first().map(String::as_str) {
        Some("check") if rest.len() == 1 => {
            let r = checks::check(Path::new(rest[0]), editor);
            print!("{}", r.render());
            std::process::exit(if r.passed() { 0 } else { 1 });
        }
        Some("serve") if args.len() == 5 => {
            // Child process for out-of-process hosting; started by `oop`.
            let block = args[4].parse().unwrap_or(256);
            if let Err(e) = oop::serve(Path::new(&args[1]), Path::new(&args[2]), &args[3], block) {
                eprintln!("plugin process: {e}");
                std::process::exit(1);
            }
        }
        Some("oop") => {
            let mut o = oop_bench::Opts {
                bundles: Vec::new(),
                seconds: 20.0,
                block: 256,
                kill: true,
            };
            let mut it = args.iter().skip(1);
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--seconds" => {
                        o.seconds = it.next().and_then(|v| v.parse().ok()).unwrap_or(20.0)
                    }
                    "--block" => o.block = it.next().and_then(|v| v.parse().ok()).unwrap_or(256),
                    "--no-kill" => o.kill = false,
                    b => o.bundles.push(PathBuf::from(b)),
                }
            }
            if o.bundles.is_empty() {
                eprintln!(
                    "usage: plugin-host oop <plugin.vst3>... [--seconds 20] [--block 256] [--no-kill]"
                );
                std::process::exit(2);
            }
            let exe = std::env::current_exe().expect("own path");
            let report = oop_bench::run(&exe, &o);
            print!("{report}");
            if std::fs::write("plugin-host-oop-report.txt", &report).is_ok() {
                println!("Saved to plugin-host-oop-report.txt");
            }
            pause();
        }
        Some("check-all") | None => {
            let mut folders = default_folders();
            folders.extend(rest.iter().map(PathBuf::from));
            let mut plugins = Vec::new();
            for f in &folders {
                find_vst3(f, &mut plugins);
            }
            let exe = std::env::current_exe().expect("own path");
            let mut report = format!(
                "VST 3 hosting check: {} plugin(s) in {:?}\n\n",
                plugins.len(),
                folders
            );
            let mut passed = 0;
            for p in &plugins {
                println!("checking {} ...", p.display());
                let (ok, text) = run_child(&exe, p, editor);
                passed += ok as usize;
                print!("{text}");
                report.push_str(&text);
                report.push('\n');
            }
            let summary = format!("\n{passed} of {} plugin(s) passed\n", plugins.len());
            print!("{summary}");
            report.push_str(&summary);
            if std::fs::write("plugin-host-report.txt", &report).is_ok() {
                println!("Saved to plugin-host-report.txt");
            }
            pause();
        }
        _ => {
            eprintln!(
                "usage: plugin-host check <plugin.vst3> [--editor]\n       plugin-host check-all [folder...] [--editor]\n       plugin-host oop <plugin.vst3>... [--seconds 20] [--block 256] [--no-kill]"
            );
            std::process::exit(2);
        }
    }
}
