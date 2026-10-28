// SPDX-License-Identifier: GPL-3.0-or-later
//! Crash reports kept on this computer (roadmap 1.8; proposal in
//! docs/research/PLATFORM_DISTRIBUTION.md). When the app hits an internal
//! error (a Rust panic), a hook writes a plain-text report into a local
//! folder: the version, the system, where the error happened and a
//! backtrace. The user's name and home folder are taken out, and so is
//! any file path in the error's message (docs/research/PRIVACY_CHILD_DATA.md,
//! item 15). Nothing is sent anywhere: at the next start the app says a
//! report was saved, shows its exact text, and offers to open the folder,
//! so the user can attach it to a bug report if they choose.
//!
//! Crashes in native code (most likely a plugin) are not caught here yet.

use std::backtrace::Backtrace;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui;

use crate::APP_NAME;

/// At most this many reports from one run, in case something panics in
/// a loop.
const MAX_PER_RUN: u32 = 3;
/// Older reports than the newest this many are deleted.
const KEEP: usize = 20;
/// Names the newest report already shown, inside the reports folder.
const SEEN_FILE: &str = "shown.txt";

/// The local folder for reports: `%LOCALAPPDATA%\DAW\Crash reports` on
/// Windows, `~/.local/state/DAW/Crash reports` elsewhere.
pub(crate) fn reports_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_STATE_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("state")))
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_NAME)
        .join("Crash reports")
}

/// Writes a report for every panic, then lets the standard hook print to
/// the terminal as before.
pub(crate) fn install(dir: PathBuf) {
    static WRITTEN: AtomicU32 = AtomicU32::new(0);
    let standard = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if WRITTEN.fetch_add(1, Ordering::Relaxed) < MAX_PER_RUN {
            let message = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "(no message)".into());
            let place = info
                .location()
                .map_or_else(|| "unknown".into(), |l| l.to_string());
            let thread = std::thread::current()
                .name()
                .unwrap_or("unnamed")
                .to_string();
            let report = Report {
                secs: now_secs(),
                thread,
                place,
                message,
                backtrace: Backtrace::force_capture().to_string(),
            };
            // A failure to write the report must not hide the panic.
            let _ = report.write(&dir, &Privacy::from_env());
        }
        standard(info);
    }));
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// What a report says, before it is written.
struct Report {
    secs: u64,
    thread: String,
    place: String,
    message: String,
    backtrace: String,
}

impl Report {
    fn text(&self, privacy: &Privacy) -> String {
        let message = privacy.scrub(&scrub_paths(&self.message));
        format!(
            "{APP_NAME} crash report\n\
             Version: {}\n\
             System: {} {}\n\
             Time: {} UTC\n\
             Thread: {}\n\
             Where: {}\n\
             What happened: {message}\n\
             \n\
             Your user name, home folder and any file paths in the message \
             above have been removed. This report has not been sent anywhere.\n\
             \n\
             Backtrace:\n{}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            utc(self.secs),
            privacy.scrub(&self.thread),
            privacy.scrub(&self.place),
            privacy.scrub(&self.backtrace),
        )
    }

    fn write(&self, dir: &Path, privacy: &Privacy) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("crash-{}-{}.txt", self.secs, std::process::id()));
        std::fs::write(&path, self.text(privacy))?;
        let mut all = list(dir);
        while all.len() > KEEP {
            let _ = std::fs::remove_file(all.remove(0));
        }
        Ok(path)
    }
}

/// The user's details to take out of a report.
struct Privacy {
    home: Option<String>,
    user: Option<String>,
}

impl Privacy {
    fn from_env() -> Privacy {
        let var = |names: &[&str]| {
            names
                .iter()
                .find_map(|n| std::env::var(n).ok())
                .filter(|v| !v.is_empty())
        };
        Privacy {
            home: var(&["USERPROFILE", "HOME"]),
            user: var(&["USERNAME", "USER", "LOGNAME"]),
        }
    }

    fn scrub(&self, text: &str) -> String {
        let mut text = text.to_string();
        if let Some(home) = &self.home {
            // Paths are written with either slash on Windows.
            for h in [home.clone(), home.replace('\\', "/")] {
                if h.len() > 1 {
                    text = text.replace(&h, "~");
                }
            }
        }
        // A very short name would match inside ordinary words.
        if let Some(user) = self.user.as_ref().filter(|u| u.chars().count() >= 3) {
            text = replace_ignoring_case(&text, user, "<user>");
        }
        text
    }
}

fn replace_ignoring_case(text: &str, find: &str, with: &str) -> String {
    let lower = text.to_lowercase();
    let find = find.to_lowercase();
    // Lower-casing can change lengths outside ASCII; then match exactly.
    if lower.len() != text.len() {
        return text.replace(&find, with);
    }
    let mut out = String::new();
    let mut at = 0;
    while let Some(i) = lower[at..].find(&find) {
        out.push_str(&text[at..at + i]);
        out.push_str(with);
        at += i + find.len();
    }
    out.push_str(&text[at..]);
    out
}

/// Replaces anything that looks like a full file path with `<path>`.
/// A path in quotes runs to the closing quote, so spaces in it are
/// covered; otherwise it runs to the next space.
fn scrub_paths(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let before = if i == 0 { None } else { Some(chars[i - 1]) };
        let starts = matches!(
            before,
            None | Some(' ' | '"' | '\'' | '(' | '=' | ':' | '`')
        ) && is_path_start(&chars[i..]);
        if !starts {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let quote = before.filter(|c| matches!(c, '"' | '\'' | '`'));
        while i < chars.len() {
            let c = chars[i];
            let ends = match quote {
                Some(q) => c == q,
                None => c.is_whitespace() || c == '"' || c == '\'' || c == ')',
            };
            if ends {
                break;
            }
            i += 1;
        }
        out.push_str("<path>");
    }
    out
}

fn is_path_start(s: &[char]) -> bool {
    match s {
        // C:\ or C:/
        [d, ':', '\\' | '/', ..] if d.is_ascii_alphabetic() => true,
        // \\server\share
        ['\\', '\\', c, ..] if c.is_alphanumeric() => true,
        // /home/... or ~/...
        ['/', c, ..] | ['~', '/', c, ..] if c.is_alphanumeric() || *c == '.' => true,
        _ => false,
    }
}

/// `2026-10-28 21:08:41` from seconds since 1970.
fn utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Days to a civil date (proleptic Gregorian calendar).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Every report in the folder, oldest first.
fn list(dir: &Path) -> Vec<PathBuf> {
    let mut all: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("crash-") && n.ends_with(".txt"))
        })
        .collect();
    all.sort_by_key(|p| report_secs(p));
    all
}

fn report_secs(p: &Path) -> (u64, String) {
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let secs = name
        .strip_prefix("crash-")
        .and_then(|n| n.split(['-', '.']).next())
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    (secs, name.to_string())
}

/// Reports not yet shown to the user, oldest first.
fn unseen(dir: &Path) -> Vec<PathBuf> {
    let seen = std::fs::read_to_string(dir.join(SEEN_FILE)).unwrap_or_default();
    let seen = seen.trim();
    let all = list(dir);
    match all
        .iter()
        .position(|p| p.file_name().is_some_and(|n| n == seen))
    {
        Some(i) => all[i + 1..].to_vec(),
        None => all,
    }
}

fn mark_seen(dir: &Path) {
    if let Some(name) = list(dir).last().and_then(|p| p.file_name()) {
        let _ = std::fs::write(dir.join(SEEN_FILE), name.to_string_lossy().as_bytes());
    }
}

/// The note at start-up and the Help → Crash reports window.
pub(crate) struct Crashes {
    dir: PathBuf,
    /// New reports since the user last looked, shown as a note.
    pub(crate) new: usize,
    pub(crate) open: bool,
    /// The report being read, and its text.
    showing: Option<(PathBuf, String)>,
}

impl Crashes {
    pub(crate) fn new(dir: PathBuf) -> Crashes {
        let new = unseen(&dir).len();
        Crashes {
            dir,
            new,
            open: false,
            showing: None,
        }
    }

    /// Opens the window on the newest report.
    pub(crate) fn show_newest(&mut self) {
        self.open = true;
        if let Some(p) = list(&self.dir).pop() {
            let text = std::fs::read_to_string(&p).unwrap_or_default();
            self.showing = Some((p, text));
        }
        self.seen();
    }

    /// The note has been read or dismissed.
    pub(crate) fn seen(&mut self) {
        mark_seen(&self.dir);
        self.new = 0;
    }

    /// The note shown at start-up after a crash.
    pub(crate) fn note(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "{APP_NAME} closed because of an internal error last time. A report \
                 was saved on this computer; nothing has been sent."
            ));
            if ui.button("Show the report").clicked() {
                self.show_newest();
            }
            if ui.button("Open the folder").clicked() {
                crate::export::open_folder(&self.dir);
                self.seen();
            }
            if ui.button("OK").clicked() {
                self.seen();
            }
        });
    }

    pub(crate) fn window(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        let mut open = self.open;
        egui::Window::new("Crash reports")
            .open(&mut open)
            .default_size([640.0, 420.0])
            .show(ctx, |ui| {
                ui.label(
                    "Reports of internal errors are kept on this computer only. To \
                     report a problem, you can attach one to a message or a bug report. \
                     Your user name, home folder and file paths are taken out first; \
                     the text below is exactly what the file holds.",
                );
                ui.horizontal(|ui| {
                    if ui.button("Open the folder").clicked() {
                        crate::export::open_folder(&self.dir);
                    }
                    ui.label(egui::RichText::new(self.dir.display().to_string()).weak());
                });
                ui.separator();
                let all = list(&self.dir);
                if all.is_empty() {
                    ui.label("No reports.");
                    return;
                }
                ui.horizontal_wrapped(|ui| {
                    for p in all.iter().rev() {
                        let label = utc(report_secs(p).0);
                        let on = self.showing.as_ref().is_some_and(|(s, _)| s == p);
                        if ui.selectable_label(on, label).clicked() {
                            let text = std::fs::read_to_string(p).unwrap_or_default();
                            self.showing = Some((p.clone(), text));
                        }
                    }
                });
                if let Some((_, text)) = &mut self.showing {
                    if ui.button("Copy the text").clicked() {
                        ui.ctx().copy_text(text.clone());
                    }
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        // Read-only: shown in a text box so it can be selected.
                        ui.add(
                            egui::TextEdit::multiline(&mut text.as_str())
                                .code_editor()
                                .desired_width(f32::INFINITY),
                        );
                    });
                }
            });
        self.open = open;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("daw-crash-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn windows_user() -> Privacy {
        Privacy {
            home: Some(r"C:\Users\Sam Smith".into()),
            user: Some("Sam Smith".into()),
        }
    }

    #[test]
    fn takes_out_the_home_folder_and_user_name() {
        let p = windows_user();
        assert_eq!(
            p.scrub(r"at C:\Users\Sam Smith\src\main.rs:3 and C:/Users/Sam Smith/x"),
            r"at ~\src\main.rs:3 and ~/x"
        );
        assert_eq!(p.scrub("thread of SAM SMITH"), "thread of <user>");
        let linux = Privacy {
            home: Some("/home/sam".into()),
            user: Some("sam".into()),
        };
        assert_eq!(linux.scrub("/home/sam/a.wav by sam"), "~/a.wav by <user>");
        // Too short to take out safely.
        let short = Privacy {
            home: None,
            user: Some("al".into()),
        };
        assert_eq!(short.scrub("totally"), "totally");
    }

    #[test]
    fn takes_out_file_paths() {
        assert_eq!(
            scrub_paths(r#"could not open "D:\Music\My Song.wav": not found"#),
            r#"could not open "<path>": not found"#
        );
        assert_eq!(
            scrub_paths("no such file /mnt/usb/lesson 3.wav, giving up"),
            "no such file <path> 3.wav, giving up"
        );
        assert_eq!(scrub_paths(r"share \\nas\music\a.wav"), "share <path>");
        assert_eq!(scrub_paths("in ~/Music/a.wav"), "in <path>");
        // Not paths.
        assert_eq!(
            scrub_paths("index 3/4 out of range: len 2, ratio 1/2"),
            "index 3/4 out of range: len 2, ratio 1/2"
        );
        assert_eq!(scrub_paths("see https://x"), "see https://x");
    }

    #[test]
    fn dates() {
        assert_eq!(utc(0), "1970-01-01 00:00:00");
        assert_eq!(utc(1_793_221_721), "2026-10-28 21:08:41");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00:00");
    }

    #[test]
    fn a_report_says_what_happened_without_personal_details() {
        let r = Report {
            secs: 1_793_221_721,
            thread: "main".into(),
            place: "crates/app/src/main.rs:10:5".into(),
            message: r#"failed to read "C:\Users\Sam Smith\DAW\Song\project.loro""#.into(),
            backtrace: r"   0: daw::main
             at C:\Users\Sam Smith\.cargo\registry\x.rs:1"
                .into(),
        };
        let text = r.text(&windows_user());
        assert!(text.contains(&format!("Version: {}", env!("CARGO_PKG_VERSION"))));
        assert!(text.contains("Time: 2026-10-28 21:08:41 UTC"));
        assert!(text.contains(r#"What happened: failed to read "<path>""#));
        assert!(text.contains(r"at ~\.cargo\registry\x.rs:1"));
        assert!(!text.contains("Sam"), "{text}");
    }

    #[test]
    fn new_reports_are_noticed_once() {
        let dir = temp("seen");
        assert_eq!(Crashes::new(dir.clone()).new, 0);
        let report = |secs| Report {
            secs,
            thread: "main".into(),
            place: "here".into(),
            message: "boom".into(),
            backtrace: String::new(),
        };
        let p = Privacy {
            home: None,
            user: None,
        };
        report(100).write(&dir, &p).unwrap();
        report(200).write(&dir, &p).unwrap();
        let mut c = Crashes::new(dir.clone());
        assert_eq!(c.new, 2);
        c.show_newest();
        assert!(c.showing.as_ref().unwrap().1.contains("boom"));
        assert_eq!(Crashes::new(dir.clone()).new, 0);
        report(300).write(&dir, &p).unwrap();
        assert_eq!(Crashes::new(dir.clone()).new, 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn old_reports_are_deleted() {
        let dir = temp("keep");
        let p = Privacy {
            home: None,
            user: None,
        };
        for secs in 0..(KEEP as u64 + 5) {
            Report {
                secs,
                thread: String::new(),
                place: String::new(),
                message: String::new(),
                backtrace: String::new(),
            }
            .write(&dir, &p)
            .unwrap();
        }
        let all = list(&dir);
        assert_eq!(all.len(), KEEP);
        assert_eq!(report_secs(&all[0]).0, 5);
        let _ = std::fs::remove_dir_all(dir);
    }
}
