// SPDX-License-Identifier: GPL-3.0-or-later
//! Exporting the mix, and each track as a stem, to WAV (roadmap 1.8).
//! Rendering runs on a thread of its own with its own copies of any
//! plugins, so playback carries on meanwhile.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;

use daw_engine::export::{Written, write_wav};
use daw_engine::graph::Graph;
use daw_engine::time::Frames;
use daw_plugins::Plugin;
use eframe::egui::{self, Color32, RichText};

/// Each file written: its name, and how it went.
type Lines = Vec<(String, Result<Written, String>)>;

/// One file to write.
pub struct Part {
    pub name: String,
    pub graph: Graph,
}

/// What to export, as chosen in the window.
pub struct Request {
    pub loop_only: bool,
    pub stems: bool,
    /// Where the new numbered folder goes.
    pub folder: PathBuf,
}

struct Job {
    thread: JoinHandle<Lines>,
    done: Arc<AtomicU64>,
    total: u64,
    cancel: Arc<AtomicBool>,
    /// The plugins' UI sides, kept until rendering ends so the plugins
    /// are shut down here rather than on the render thread.
    _plugins: Vec<Plugin>,
    folder: PathBuf,
}

#[derive(Default)]
pub struct Export {
    pub open: bool,
    loop_only: bool,
    stems: bool,
    /// Where exports go unless the user chooses: the project's own
    /// Exports folder once it is saved.
    default_dir: PathBuf,
    /// A folder the user chose for this project instead.
    chosen: Option<PathBuf>,
    job: Option<Job>,
    /// The last export's folder, and a line about each file.
    result: Option<(PathBuf, Lines)>,
}

/// A new numbered folder under `base`: `Mix 1`, `Mix 2`, …
fn next_folder(base: &Path) -> PathBuf {
    (1..)
        .map(|n| base.join(format!("Mix {n}")))
        .find(|p| !p.exists())
        .unwrap()
}

/// A track name made safe for a file name.
pub fn file_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_.()".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let s = s.trim().trim_matches('.').to_string();
    if s.is_empty() { "Track".into() } else { s }
}

impl Export {
    /// Sets where exports go by default. A folder the user chose is
    /// forgotten when this changes (another project was opened).
    pub fn set_default_dir(&mut self, dir: PathBuf) {
        if dir != self.default_dir {
            self.default_dir = dir;
            self.chosen = None;
        }
    }

    fn dir(&self) -> &Path {
        self.chosen.as_deref().unwrap_or(&self.default_dir)
    }

    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Starts writing `parts` into a new folder under `base`, from
    /// `from` for `frames` frames each.
    pub fn start(
        &mut self,
        base: &Path,
        parts: Vec<Part>,
        plugins: Vec<Plugin>,
        from: Frames,
        frames: usize,
    ) -> Result<(), String> {
        let folder = next_folder(base);
        std::fs::create_dir_all(&folder)
            .map_err(|e| format!("could not make {}: {e}", folder.display()))?;
        let done = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let total = (frames * parts.len()) as u64;
        let (d, c, f) = (done.clone(), cancel.clone(), folder.clone());
        let thread = std::thread::spawn(move || {
            let mut out = Vec::new();
            for p in parts {
                let path = f.join(format!("{}.wav", p.name));
                let r = write_wav(p.graph, from, frames, &path, &d, &c);
                out.push((p.name, r));
                if c.load(Ordering::Relaxed) {
                    break;
                }
            }
            out
        });
        self.result = None;
        self.job = Some(Job {
            thread,
            done,
            total,
            cancel,
            _plugins: plugins,
            folder,
        });
        Ok(())
    }

    /// Checks on a running export.
    pub fn poll(&mut self) {
        if self.job.as_ref().is_some_and(|j| j.thread.is_finished()) {
            let job = self.job.take().unwrap();
            let lines = job
                .thread
                .join()
                .unwrap_or_else(|_| vec![("Export".into(), Err("the export stopped".into()))]);
            self.result = Some((job.folder, lines));
        }
    }

    /// Draws the window. Returns a request when the user presses Export.
    pub fn window(&mut self, ctx: &egui::Context, loop_text: &str) -> Option<Request> {
        let mut open = self.open;
        let mut request = None;
        egui::Window::new("Export")
            .open(&mut open)
            .resizable(false)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.add_enabled_ui(!self.busy(), |ui| {
                    ui.radio_value(&mut self.loop_only, false, "The whole song");
                    ui.radio_value(&mut self.loop_only, true, format!("The loop ({loop_text})"));
                    ui.checkbox(&mut self.stems, "Also each track on its own (stems)");
                    ui.label(
                        RichText::new(
                            "24-bit WAV at the session's rate. Practice mute and the \
                             metronome are left out.",
                        )
                        .small()
                        .weak(),
                    );
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(format!("Into {}", self.dir().display())).small());
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Choose folder…").clicked()
                            && let Some(d) = rfd::FileDialog::new()
                                .set_title("Export into")
                                .set_directory(self.dir())
                                .pick_folder()
                        {
                            self.chosen = Some(d);
                        }
                        if self.chosen.is_some() && ui.button("Back to the usual folder").clicked() {
                            self.chosen = None;
                        }
                    });
                });
                ui.add_space(6.0);
                match &self.job {
                    Some(job) => {
                        ui.horizontal(|ui| {
                            let f = job.done.load(Ordering::Relaxed) as f32 / job.total.max(1) as f32;
                            ui.add(egui::ProgressBar::new(f).show_percentage().desired_width(220.0));
                            if ui.button("Cancel").clicked() {
                                job.cancel.store(true, Ordering::Relaxed);
                            }
                        });
                    }
                    None => {
                        if ui.button("Export").clicked() {
                            request = Some(Request {
                                loop_only: self.loop_only,
                                stems: self.stems,
                                folder: self.dir().to_path_buf(),
                            });
                        }
                    }
                }
                if let Some((folder, lines)) = &self.result {
                    ui.separator();
                    for (name, r) in lines {
                        match r {
                            Ok(w) if w.clipped() => {
                                ui.colored_label(
                                    Color32::YELLOW,
                                    format!(
                                        "{name}.wav: went {:.1} dB over and was clipped; turn it down",
                                        20.0 * w.peak.log10()
                                    ),
                                );
                            }
                            Ok(_) => {
                                ui.label(format!("{name}.wav ✔"));
                            }
                            Err(e) => {
                                ui.colored_label(Color32::LIGHT_RED, format!("{name}: {e}"));
                            }
                        }
                    }
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(folder.display().to_string()).small());
                        if ui.button("Open folder").clicked() {
                            open_folder(folder);
                        }
                    });
                }
            });
        self.open = open;
        request
    }
}

fn open_folder(path: &Path) {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(program).arg(path).spawn();
}
