// SPDX-License-Identifier: GPL-3.0-or-later
//! Exporting the mix, each track and each bus as a stem, to WAV, FLAC,
//! MP3 or Opus (roadmap 1.8, 3.6). Rendering runs on a thread of its own with its own copies of any
//! plugins, so playback carries on meanwhile.
//!
//! The mix can also go into a copy of a video clip's film, as its sound
//! (roadmap 3.10): the mix is rendered for the length of the film from
//! its first frame, and the system's media framework writes the copy
//! without re-encoding the picture.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;

use daw_engine::export::{Format, MP3_KBPS, OPUS_KBPS, Written, write};
use daw_engine::graph::Graph;
use daw_engine::time::Frames;
use daw_plugins::Plugin;
use eframe::egui::{self, RichText};

/// Each file written: its file name, and how it went.
type Lines = Vec<(String, Result<Written, String>)>;

/// A file written, and what went wrong if anything did.
// Only the video tests, which need a system that reads video, use it.
#[cfg(all(test, any(target_os = "linux", windows)))]
pub type Outcome<'a> = (&'a str, Option<&'a str>);

/// One file to write.
pub struct Part {
    pub name: String,
    pub graph: Graph,
    /// Rendered from here, for this many frames.
    pub from: Frames,
    pub frames: usize,
    /// A video file to put the sound into a copy of, in place of a
    /// sound file.
    pub film: Option<PathBuf>,
}

/// What to export, as chosen in the window.
pub struct Request {
    pub loop_only: bool,
    pub stems: bool,
    /// Also each bus, as it sounds in the mix.
    pub bus_stems: bool,
    pub format: Format,
    /// Where the new numbered folder goes.
    pub folder: PathBuf,
    /// The mix on its own, in `format`.
    pub mix: bool,
    /// The mix in a copy of the video.
    pub video: bool,
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
    bus_stems: bool,
    video: bool,
    kind: Kind,
    mp3_kbps: Option<u32>,
    opus_kbps: Option<u32>,
    /// Where exports go unless the user chooses: the project's own
    /// Exports folder once it is saved.
    default_dir: PathBuf,
    /// A folder the user chose for this project instead.
    chosen: Option<PathBuf>,
    job: Option<Job>,
    /// The last export's folder, and a line about each file.
    result: Option<(PathBuf, Lines)>,
}

/// The kinds of file offered, in the order shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Kind {
    #[default]
    Wav,
    Flac,
    Mp3,
    Opus,
}

impl Kind {
    const ALL: [Kind; 4] = [Kind::Wav, Kind::Flac, Kind::Mp3, Kind::Opus];

    fn label(self) -> &'static str {
        match self {
            Kind::Wav => "WAV",
            Kind::Flac => "FLAC",
            Kind::Mp3 => "MP3",
            Kind::Opus => "Opus",
        }
    }

    fn note(self) -> &'static str {
        match self {
            Kind::Wav => "24-bit WAV at the session's rate.",
            Kind::Flac => {
                "24-bit FLAC at the session's rate: the same sound as WAV, in about \
                 half the space."
            }
            Kind::Mp3 => {
                "MP3 plays almost anywhere. It is much smaller, and loses a little detail."
            }
            Kind::Opus => {
                "Opus sounds better than MP3 at the same size, at 48 kHz. Most \
                 players and browsers open it; some older ones don't."
            }
        }
    }
}

/// Usual bit rates: high enough that a mix sounds as it should.
const MP3_USUAL: u32 = 320;
const OPUS_USUAL: u32 = 160;

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

    /// Starts writing `parts` into a new folder under `base`.
    pub fn start(
        &mut self,
        base: &Path,
        parts: Vec<Part>,
        plugins: Vec<Plugin>,
        format: Format,
    ) -> Result<(), String> {
        let folder = next_folder(base);
        std::fs::create_dir_all(&folder)
            .map_err(|e| format!("could not make {}: {e}", folder.display()))?;
        let done = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let total = parts.iter().map(|p| p.frames as u64).sum();
        let (d, c, f) = (done.clone(), cancel.clone(), folder.clone());
        let thread = std::thread::spawn(move || {
            let mut out = Vec::new();
            for p in parts {
                let (name, r) = match &p.film {
                    None => {
                        let name = format!("{}.{}", p.name, format.extension());
                        let r = write(p.graph, p.from, p.frames, &f.join(&name), format, &d, &c);
                        (name, r)
                    }
                    Some(film) => {
                        let name = format!("{}.{}", p.name, daw_video::wrapped_extension(film));
                        // The mix as WAV first, wrapped in the copy, then
                        // let go of.
                        let sound = f.join(format!(".{}.mix.wav", p.name));
                        let r = write(p.graph, p.from, p.frames, &sound, Format::Wav, &d, &c)
                            .and_then(|w| {
                                daw_video::wrap(film, &sound, &f.join(&name), &c)
                                    .map(|()| w)
                                    .map_err(|e| e.to_string())
                            });
                        let _ = std::fs::remove_file(&sound);
                        (name, r)
                    }
                };
                out.push((name, r));
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

    /// What Export the video with the mix asks for: only that, as the
    /// window is set otherwise.
    pub fn video_request(&self) -> Request {
        Request {
            loop_only: false,
            stems: false,
            bus_stems: false,
            format: self.format(),
            folder: self.dir().to_path_buf(),
            mix: false,
            video: true,
        }
    }

    /// The last export's folder and its files, each with what went
    /// wrong if it did, once it has finished.
    #[cfg(all(test, any(target_os = "linux", windows)))]
    pub fn finished(&self) -> Option<(&Path, Vec<Outcome<'_>>)> {
        let (folder, lines) = self.result.as_ref()?;
        let lines = (lines.iter())
            .map(|(n, r)| (n.as_str(), r.as_ref().err().map(String::as_str)))
            .collect();
        Some((folder, lines))
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

    fn format(&self) -> Format {
        match self.kind {
            Kind::Wav => Format::Wav,
            Kind::Flac => Format::Flac,
            Kind::Mp3 => Format::Mp3(self.mp3_kbps.unwrap_or(MP3_USUAL)),
            Kind::Opus => Format::Opus(self.opus_kbps.unwrap_or(OPUS_USUAL)),
        }
    }

    /// Draws the window. Returns a request when the user presses Export.
    /// `buses` says whether the session has any buses to export.
    /// `film` names the video the mix can go into a copy of, if there is
    /// one.
    pub fn window(
        &mut self,
        ctx: &egui::Context,
        loop_text: &str,
        buses: bool,
        film: Option<&str>,
    ) -> Option<Request> {
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
                    ui.add_enabled_ui(buses, |ui| {
                        ui.checkbox(
                            &mut self.bus_stems,
                            "Also each bus, as it sounds in the mix",
                        )
                        .on_hover_text(
                            "Everything that goes through the bus, after its effects \
                                 and fader",
                        )
                        .on_disabled_hover_text("This song has no buses");
                    });
                    ui.add_enabled_ui(film.is_some() && daw_video::CAN_WRAP, |ui| {
                        let label = match film {
                            Some(name) => {
                                format!("Also a copy of the video \"{name}\" with this mix")
                            }
                            None => "Also a copy of the video with this mix".into(),
                        };
                        ui.checkbox(&mut self.video, label)
                            .on_hover_text(
                                "The whole film with the mix as its sound, from its first \
                                 frame. The picture is copied as it is, not re-encoded",
                            )
                            .on_disabled_hover_text(if daw_video::CAN_WRAP {
                                "This song has no video"
                            } else {
                                "Not on this system yet"
                            });
                    });
                    ui.horizontal(|ui| {
                        let l = ui.label("File type");
                        egui::ComboBox::from_id_salt("export-kind")
                            .selected_text(self.kind.label())
                            .show_ui(ui, |ui| {
                                for k in Kind::ALL {
                                    ui.selectable_value(&mut self.kind, k, k.label());
                                }
                            })
                            .response
                            .labelled_by(l.id);
                        let rates = match self.kind {
                            Kind::Mp3 => Some((&mut self.mp3_kbps, MP3_USUAL, &MP3_KBPS[..])),
                            Kind::Opus => Some((&mut self.opus_kbps, OPUS_USUAL, &OPUS_KBPS[..])),
                            _ => None,
                        };
                        if let Some((chosen, usual, rates)) = rates {
                            let mut kbps = chosen.unwrap_or(usual);
                            egui::ComboBox::from_id_salt("export-kbps")
                                .selected_text(format!("{kbps} kbit/s"))
                                .show_ui(ui, |ui| {
                                    for &r in rates {
                                        ui.selectable_value(&mut kbps, r, format!("{r} kbit/s"));
                                    }
                                })
                                .response
                                .on_hover_text(
                                    "Bit rate: higher sounds better and makes a bigger file",
                                )
                                .labelled_by(l.id);
                            *chosen = Some(kbps);
                        }
                    });
                    ui.label(
                        RichText::new(format!(
                            "{} Practice mute and the metronome are left out.",
                            self.kind.note()
                        ))
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
                        if self.chosen.is_some() && ui.button("Back to the usual folder").clicked()
                        {
                            self.chosen = None;
                        }
                    });
                });
                ui.add_space(6.0);
                match &self.job {
                    Some(job) => {
                        ui.horizontal(|ui| {
                            let f =
                                job.done.load(Ordering::Relaxed) as f32 / job.total.max(1) as f32;
                            ui.add(
                                egui::ProgressBar::new(f)
                                    .show_percentage()
                                    .desired_width(220.0),
                            );
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
                                bus_stems: self.bus_stems && buses,
                                format: self.format(),
                                folder: self.dir().to_path_buf(),
                                mix: true,
                                video: self.video && film.is_some() && daw_video::CAN_WRAP,
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
                                    ui.visuals().warn_fg_color,
                                    format!(
                                        "{name}: went {:.1} dB over and was clipped; turn it down",
                                        20.0 * w.peak.log10()
                                    ),
                                );
                            }
                            Ok(_) => {
                                ui.label(format!("{name} ✔"));
                            }
                            Err(e) => {
                                ui.colored_label(
                                    ui.visuals().error_fg_color,
                                    format!("{name}: {e}"),
                                );
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

pub(crate) fn open_folder(path: &Path) {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(program).arg(path).spawn();
}
