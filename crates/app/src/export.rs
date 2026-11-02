// SPDX-License-Identifier: GPL-3.0-or-later
//! Exporting the mix, each track and each bus as a stem, to WAV, FLAC,
//! MP3 or Opus (roadmap 1.8, 3.6). Rendering runs on a thread of its own with its own copies of any
//! plugins, so playback carries on meanwhile.
//!
//! The mix can also go into a copy of a video clip's film, as its sound
//! (roadmap 3.10): the mix is rendered for the length of the film from
//! its first frame, and the system's media framework writes the copy
//! without re-encoding the picture.
//!
//! A practice file can be made for each song section (Phase 4, B3): one
//! file per section, cut exactly at its bars so it loops cleanly, and if
//! asked without the tracks muted for practice, to play along with.
//! Exports wait in a queue and run one after another, so several can be
//! set going at once.

use crate::icons::{Icon, IconButton};
use std::collections::VecDeque;
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
    /// Only these tracks' and buses' stems, by key; `None` for every
    /// one.
    pub only: Option<Vec<String>>,
    /// These bars, from the start of the first to the start of the
    /// second, in place of the loop or the whole song.
    pub bars: Option<(i64, i64)>,
    /// A stem package, in place of an ordinary export.
    pub package: Option<Package>,
    /// A file of the mix for each of these, in place of one mix.
    pub sections: Vec<Span>,
    /// Leave out the tracks muted for practice, so the file can be
    /// played along with.
    pub play_along: bool,
}

/// What a stem package adds to the audio files.
#[derive(Clone, Debug, Default)]
pub struct Package {
    /// Files written beside the audio: the notes as MIDI files and a
    /// page about the stems. Name and contents.
    pub files: Vec<(String, Vec<u8>)>,
    /// Packed into one `.tar` file, in place of the folder.
    pub archive: bool,
}

/// A named run of bars: a song section, or the loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub name: String,
    /// From the start of bar `from` to the start of bar `to`.
    pub from: i64,
    pub to: i64,
}

impl Request {
    /// What the export is, in a few words, for the queue.
    pub fn label(&self) -> String {
        let what = match self.sections.as_slice() {
            [] if self.video && !self.mix => "The video with the mix".to_string(),
            [] => match self.bars {
                Some((a, b)) => format!("Bars {a} to {b}"),
                None if self.loop_only => "The loop".into(),
                None => "The whole song".into(),
            },
            [one] => format!("“{}” (bars {} to {})", one.name, one.from, one.to),
            many => format!("{} sections, a file each", many.len()),
        };
        let along = if self.play_along {
            ", to play along with"
        } else {
            ""
        };
        format!("{what}{along}, {}", self.format.extension().to_uppercase())
    }
}

/// An export waiting its turn.
pub struct Queued {
    pub label: String,
    pub request: Request,
}

/// What the window asks for.
pub enum Action {
    /// Start this now (or as soon as the one running finishes).
    Export(Request),
    /// Put this at the end of the queue.
    Queue(Request),
}

/// One export's folder, and how each of its files went.
pub type Done = (PathBuf, Lines);

struct Job {
    thread: JoinHandle<Lines>,
    done: Arc<AtomicU64>,
    total: u64,
    cancel: Arc<AtomicBool>,
    /// The plugins' UI sides, kept until rendering ends so the plugins
    /// are shut down here rather than on the render thread.
    _plugins: Vec<Plugin>,
    folder: PathBuf,
    started: std::time::Instant,
}

#[derive(Default)]
pub struct Export {
    pub open: bool,
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
    /// Each export finished since the queue last started from empty:
    /// its folder, and a line about each file.
    results: Vec<Done>,
    /// How many exports have finished (or failed to start) since the
    /// app started: the last of `results` is this one.
    finished: u64,
    /// Exports waiting for the one running to finish.
    queue: VecDeque<Queued>,
    /// What the window exports: the whole song, the loop, or a file per
    /// section.
    what: What,
    /// Sections left out of a file-per-section export, by marker id.
    skipped: std::collections::HashSet<String>,
    play_along: bool,
    /// The next start is the queue's, so it keeps the run's lines.
    from_queue: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum What {
    #[default]
    Song,
    Loop,
    Sections,
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

/// A new numbered folder under `base`: `Mix 1`, `Mix 2`, … (or
/// `Stems 1`, … for a stem package, with no archive of that name
/// either, or `Sections 1`, … for practice files).
fn next_folder(base: &Path, word: &str) -> PathBuf {
    (1..)
        .map(|n| base.join(format!("{word} {n}")))
        .find(|p| !p.exists() && !p.with_extension("tar").exists())
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

    /// Sets a folder chosen for this project, or goes back to the usual
    /// one.
    pub fn chosen(&self) -> Option<&Path> {
        self.chosen.as_deref()
    }

    pub fn set_chosen(&mut self, dir: Option<PathBuf>) {
        self.chosen = dir;
    }

    pub fn dir(&self) -> &Path {
        self.chosen.as_deref().unwrap_or(&self.default_dir)
    }

    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Starts writing `parts` into a new folder under `base`, named
    /// with `word` and a number, and the rest of a stem package if
    /// there is one.
    pub fn start(
        &mut self,
        base: &Path,
        word: &str,
        parts: Vec<Part>,
        plugins: Vec<Plugin>,
        format: Format,
        package: Option<Package>,
    ) -> Result<(), String> {
        let folder = next_folder(base, word);
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
            if let Some(pkg) = package
                && !c.load(Ordering::Relaxed)
            {
                for (name, bytes) in &pkg.files {
                    let r = std::fs::write(f.join(name), bytes)
                        .map(|()| Written { peak: 0.0 })
                        .map_err(|e| e.to_string());
                    out.push((name.clone(), r));
                }
                if pkg.archive {
                    let dest = f.with_extension("tar");
                    let r = daw_collab::archive::pack(&f, &dest)
                        .and_then(|()| std::fs::remove_dir_all(&f).map_err(|e| e.to_string()))
                        .map(|()| Written { peak: 0.0 });
                    let name = dest.file_name().unwrap_or_default().to_string_lossy();
                    out.push((name.into_owned(), r));
                }
            }
            out
        });
        if !std::mem::take(&mut self.from_queue) {
            // A new run: the last run's lines go.
            self.results.clear();
        }
        self.job = Some(Job {
            thread,
            done,
            total,
            cancel,
            _plugins: plugins,
            folder,
            started: std::time::Instant::now(),
        });
        Ok(())
    }

    /// The mix of the whole song, as the window is set otherwise (its
    /// file type, folder, stems and play-along), without the video.
    pub fn mix_request(&self) -> Request {
        Request {
            loop_only: false,
            stems: self.stems,
            bus_stems: self.bus_stems,
            format: self.format(),
            folder: self.dir().to_path_buf(),
            mix: true,
            video: false,
            only: None,
            bars: None,
            package: None,
            sections: Vec::new(),
            play_along: self.play_along,
        }
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
            only: None,
            bars: None,
            package: None,
            sections: Vec::new(),
            play_along: false,
        }
    }

    /// A file of the mix for each of `sections`, in the window's file
    /// type and folder.
    pub fn sections_request(&self, sections: Vec<Span>, play_along: bool) -> Request {
        Request {
            loop_only: false,
            stems: false,
            bus_stems: false,
            format: self.format(),
            folder: self.dir().to_path_buf(),
            mix: false,
            video: false,
            only: None,
            bars: None,
            package: None,
            sections,
            play_along,
        }
    }

    /// Whether the window is set to leave out tracks muted for practice.
    pub fn play_along(&self) -> bool {
        self.play_along
    }

    /// Puts `request` at the end of the queue.
    pub fn enqueue(&mut self, request: Request) {
        let label = request.label();
        self.queue.push_back(Queued { label, request });
    }

    /// The next export to start, if nothing is running.
    pub fn next_queued(&mut self) -> Option<Queued> {
        if self.busy() {
            return None;
        }
        let q = self.queue.pop_front()?;
        self.from_queue = true;
        Some(q)
    }

    pub fn queue(&self) -> impl Iterator<Item = &Queued> {
        self.queue.iter()
    }

    /// Stops the export running, if one is: what it has written so far
    /// is kept.
    pub fn cancel(&mut self) -> bool {
        match &self.job {
            Some(job) => {
                job.cancel.store(true, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    /// Takes the export `i` (from 0) out of the queue.
    pub fn unqueue(&mut self, i: usize) -> bool {
        self.queue.remove(i).is_some()
    }

    pub fn clear_queue(&mut self) {
        self.queue.clear();
    }

    /// Notes a queued export that could not start.
    pub fn failed(&mut self, label: String, error: String) {
        if !std::mem::take(&mut self.from_queue) {
            self.results.clear();
        }
        self.results
            .push((PathBuf::new(), vec![(label, Err(error))]));
        self.finished += 1;
    }

    /// Each export finished in this run, oldest first.
    pub fn results(&self) -> &[Done] {
        &self.results
    }

    /// How many exports have finished since the app started, counting
    /// those that couldn't start.
    pub fn finished_count(&self) -> u64 {
        self.finished
    }

    /// The `n`th export to finish since the app started (from 1), while
    /// it is still among this run's results.
    pub fn numbered(&self, n: u64) -> Option<&Done> {
        let first = self.finished + 1 - self.results.len() as u64;
        let i = n.checked_sub(first)?;
        self.results.get(i as usize)
    }

    /// How many exports must finish before one asked for now: the one
    /// running and those queued.
    pub fn ahead(&self) -> u64 {
        u64::from(self.busy()) + self.queue.len() as u64
    }

    /// How far the running export has got, from 0 to 1.
    pub fn progress(&self) -> Option<f32> {
        let j = self.job.as_ref()?;
        Some(j.done.load(Ordering::Relaxed) as f32 / j.total.max(1) as f32)
    }

    /// How long the running export has taken so far, in seconds.
    pub fn elapsed(&self) -> Option<f64> {
        Some(self.job.as_ref()?.started.elapsed().as_secs_f64())
    }

    /// The last export's folder and its files, each with what went
    /// wrong if it did, once it has finished.
    #[cfg(all(test, any(target_os = "linux", windows)))]
    pub fn finished(&self) -> Option<(&Path, Vec<Outcome<'_>>)> {
        let (folder, lines) = self.results.last()?;
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
            self.results.push((job.folder, lines));
            self.finished += 1;
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

    /// Draws the window. Returns what the user asks for, if they press
    /// Export or Add to queue. `buses` says whether the session has any
    /// buses to export. `film` names the video the mix can go into a
    /// copy of, if there is one. `sections` are the song's sections.
    pub fn window(
        &mut self,
        ctx: &egui::Context,
        loop_text: &str,
        buses: bool,
        film: Option<&str>,
        sections: &[daw_project::Section],
    ) -> Option<Action> {
        let mut open = self.open;
        let mut action = None;
        if sections.is_empty() && self.what == What::Sections {
            self.what = What::Song;
        }
        egui::Window::new("Export")
            .open(&mut open)
            .resizable(false)
            .default_width(360.0)
            .show(ctx, |ui| {
                // ai: export_mix
                ui.radio_value(&mut self.what, What::Song, "The whole song");
                // ai: export_mix.loop
                ui.radio_value(
                    &mut self.what,
                    What::Loop,
                    format!("The loop ({loop_text})"),
                );
                ui.add_enabled_ui(!sections.is_empty(), |ui| {
                    // ai: export_sections
                    ui.radio_value(
                        &mut self.what,
                        What::Sections,
                        "A practice file for each section",
                    )
                    .on_hover_text(
                        "One file per section, cut at its bars so it loops cleanly in any \
                         player",
                    )
                    .on_disabled_hover_text("Mark sections first: M marks one at the playhead");
                });
                let per_section = self.what == What::Sections;
                if per_section {
                    ui.indent("export-sections", |ui| {
                        for s in sections {
                            let mut on = !self.skipped.contains(&s.id);
                            let text = format!("{} (bars {} to {})", s.name, s.from, s.to);
                            // ai: export_sections.sections
                            if ui.checkbox(&mut on, text).changed() {
                                if on {
                                    self.skipped.remove(&s.id);
                                } else {
                                    self.skipped.insert(s.id.clone());
                                }
                            }
                        }
                    });
                }
                // ai: export_mix.play_along
                ui.checkbox(
                    &mut self.play_along,
                    "Leave out the tracks muted for practice, to play along with",
                )
                .on_hover_text("Tracks with practice mute (P) on are not in the file");
                ui.add_enabled_ui(!per_section, |ui| {
                    // ai: export_mix.stems
                    ui.checkbox(&mut self.stems, "Also each track on its own (stems)")
                        .on_hover_text(
                            "Each through the master's effects and fader, as in the mix",
                        );
                    ui.add_enabled_ui(buses, |ui| {
                        // ai: export_mix.bus_stems
                        ui.checkbox(
                            &mut self.bus_stems,
                            "Also each bus, as it sounds in the mix",
                        )
                        .on_hover_text(
                            "Everything that goes through the bus, after its effects \
                                 and fader, then through the master",
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
                        // ai: export-video
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
                });
                ui.horizontal(|ui| {
                    let l = ui.label("File type");
                    egui::ComboBox::from_id_salt("export-kind")
                        .selected_text(self.kind.label())
                        .show_ui(ui, |ui| {
                            for k in Kind::ALL {
                                // ai: export_mix.format
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
                                    // ai: export_mix.kbps
                                    ui.selectable_value(&mut kbps, r, format!("{r} kbit/s"));
                                }
                            })
                            .response
                            .on_hover_text("Bit rate: higher sounds better and makes a bigger file")
                            .labelled_by(l.id);
                        *chosen = Some(kbps);
                    }
                });
                let left_out = if self.play_along {
                    "Tracks muted for practice and the metronome are left out."
                } else {
                    "Practice mute and the metronome are left out."
                };
                ui.label(
                    RichText::new(format!("{} {left_out}", self.kind.note()))
                        .small()
                        .weak(),
                );
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("Into {}", self.dir().display())).small());
                });
                ui.horizontal(|ui| {
                    // ai: export_mix.folder
                    if ui.button("Choose folder…").clicked()
                        && let Some(d) = rfd::FileDialog::new()
                            .set_title("Export into")
                            .set_directory(self.dir())
                            .pick_folder()
                    {
                        self.chosen = Some(d);
                    }
                    // ai: export_mix.folder
                    if self.chosen.is_some() && ui.button("Back to the usual folder").clicked() {
                        self.chosen = None;
                    }
                });
                ui.add_space(6.0);
                let chosen: Vec<Span> = (sections.iter())
                    .filter(|s| !self.skipped.contains(&s.id))
                    .map(|s| Span {
                        name: s.name.clone(),
                        from: s.from,
                        to: s.to,
                    })
                    .collect();
                let ready = !per_section || !chosen.is_empty();
                let request = || Request {
                    loop_only: self.what == What::Loop,
                    stems: self.stems && !per_section,
                    bus_stems: self.bus_stems && buses && !per_section,
                    format: self.format(),
                    folder: self.dir().to_path_buf(),
                    mix: !per_section,
                    video: self.video && film.is_some() && daw_video::CAN_WRAP && !per_section,
                    only: None,
                    bars: None,
                    package: None,
                    sections: if per_section {
                        chosen.clone()
                    } else {
                        Vec::new()
                    },
                    play_along: self.play_along,
                };
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(ready, |ui| {
                        // ai: export_mix
                        if !self.busy() && ui.button("Export").clicked() {
                            action = Some(Action::Export(request()));
                        }
                        if ui
                            // ai: export_mix
                            .button("Add to queue")
                            .on_hover_text(
                                "Exports wait in the queue and run one after another, with \
                                 the song as it is when each starts",
                            )
                            .clicked()
                        {
                            action = Some(Action::Queue(request()));
                        }
                    });
                });
                if let Some(job) = &self.job {
                    ui.horizontal(|ui| {
                        let f = job.done.load(Ordering::Relaxed) as f32 / job.total.max(1) as f32;
                        ui.add(
                            egui::ProgressBar::new(f)
                                .show_percentage()
                                .desired_width(220.0),
                        );
                        // ai: manage_exports.cancel
                        if ui.button("Cancel").clicked() {
                            job.cancel.store(true, Ordering::Relaxed);
                        }
                    });
                }
                if !self.queue.is_empty() {
                    ui.separator();
                    ui.label(RichText::new("Waiting to export").strong());
                    let mut gone = None;
                    for (i, q) in self.queue.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(format!("{}. {}", i + 1, q.label));
                            // ai: manage_exports.remove
                            if ui
                                .add(
                                    IconButton::new(Icon::Close, "Take it out of the queue")
                                        .small(),
                                )
                                .clicked()
                            {
                                gone = Some(i);
                            }
                        });
                    }
                    if let Some(i) = gone {
                        self.queue.remove(i);
                    }
                    // ai: clear-export-queue
                    if ui.button("Clear the queue").clicked() {
                        self.queue.clear();
                    }
                }
                for (folder, lines) in &self.results {
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
                                ui.horizontal(|ui| {
                                    ui.label(name.as_str());
                                    crate::icons::show(ui, crate::icons::Icon::Check, "done");
                                });
                            }
                            Err(e) => {
                                ui.colored_label(
                                    ui.visuals().error_fg_color,
                                    format!("{name}: {e}"),
                                );
                            }
                        }
                    }
                    if !folder.as_os_str().is_empty() {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(folder.display().to_string()).small());
                            // ai: view
                            if ui.button("Open folder").clicked() {
                                open_folder(folder);
                            }
                        });
                    }
                }
            });
        self.open = open;
        action
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

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use serde_json::{Value, json};

    use crate::agent::driver::Driver;

    /// `secs` seconds of a tone, mono, at `rate`.
    fn tone(dir: &Path, rate: u32, secs: u32) -> PathBuf {
        let path = dir.join("band.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..rate * secs {
            let x = (i as f32 * 220.0 * std::f32::consts::TAU / rate as f32).sin();
            w.write_sample((x * 16_000.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        path
    }

    /// Frames in a file, and its loudest sample.
    fn read(path: &Path) -> (u32, i32) {
        let mut r = hound::WavReader::open(path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
        let frames = r.duration();
        let peak = (r.samples::<i32>())
            .map(|s| s.unwrap().abs())
            .max()
            .unwrap_or(0);
        (frames, peak)
    }

    /// Asks for the exports until none is running or waiting.
    fn wait(d: &mut Driver) -> Value {
        for _ in 0..2000 {
            let v = d.call("get_exports", json!({})).unwrap();
            if v["running"].is_null() && v["queue"].as_array().unwrap().is_empty() {
                return v;
            }
            if !v["running"].is_null() {
                assert!(
                    v["running"]["seconds_so_far"].as_f64().unwrap() >= 0.0,
                    "{v}"
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the exports did not finish");
    }

    #[test]
    fn a_practice_file_per_section_through_the_agent_and_the_queue() {
        let dir = std::env::temp_dir().join(format!("daw-sections-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut d = Driver::new();
        let rate = d.app().rate;
        // Four bars at 120 bpm in 4/4: two seconds a bar.
        d.app().add_file(&tone(&dir, rate, 8));
        for _ in 0..1000 {
            if !d.app().tracks.is_empty() {
                break;
            }
            d.h.step();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(d.app().tracks.len(), 1);
        // Nothing to export per section until there are sections.
        let e = d
            .call("export_sections", json!({"folder": dir}))
            .unwrap_err();
        assert!(e.contains("no sections"), "{e}");
        for (bar, name) in [(1, "Verse"), (3, "Chorus")] {
            d.call("seek", json!({"bar": bar})).unwrap();
            d.command("marker");
            d.app().markers.last_mut().unwrap().name = name.into();
        }
        let out = dir.join("out");
        // Every section; then the chorus alone, to play along with, which
        // waits its turn.
        // Both asked for in the same frame, so the second must wait.
        let ask = |d: &mut Driver, args: Value| {
            let (reply, answer) = std::sync::mpsc::channel();
            let req = crate::agent::Request {
                tool: "export_sections".into(),
                args,
                access: crate::agent::Access::EditExport,
                reply,
            };
            d.app().agent.inbox.send(req).unwrap();
            answer
        };
        let a = ask(&mut d, json!({"format": "wav", "folder": out}));
        d.app().tracks[0].practice_mute = true;
        let b = ask(
            &mut d,
            json!({"sections": ["Chorus"], "play_along": true, "format": "wav", "folder": out}),
        );
        d.h.step();
        d.h.step();
        let (a, b) = (a.recv().unwrap().unwrap(), b.recv().unwrap().unwrap());
        assert_eq!(a["export"], "2 sections, a file each, WAV");
        assert_eq!(a["queued"], false);
        assert_eq!(
            b["export"],
            "“Chorus” (bars 3 to 5), to play along with, WAV"
        );
        assert_eq!(b["queued"], true);
        // The folder given was for those exports only.
        assert_ne!(d.app().export.dir(), out);

        let v = wait(&mut d);
        let done = v["finished"].as_array().unwrap();
        assert_eq!(done.len(), 2, "{v}");
        let files = |i: usize| -> Vec<String> {
            (done[i]["files"].as_array().unwrap().iter())
                .map(|f| {
                    assert!(f["error"].is_null(), "{f}");
                    f["name"].as_str().unwrap().to_string()
                })
                .collect()
        };
        assert_eq!(files(0), ["01 Verse.wav", "02 Chorus.wav"]);
        assert_eq!(files(1), ["01 Chorus.wav"]);
        let first = out.join("Sections 1");
        let second = out.join("Sections 2");
        assert_eq!(done[0]["folder"], first.display().to_string());
        // Each is cut at its bars: two bars, four seconds.
        let (frames, peak) = read(&first.join("01 Verse.wav"));
        assert_eq!(frames, 4 * rate);
        assert!(peak > 0);
        assert_eq!(read(&first.join("02 Chorus.wav")).0, 4 * rate);
        // To play along with: the practice-muted track is left out.
        let (frames, peak) = read(&second.join("01 Chorus.wav"));
        assert_eq!(frames, 4 * rate);
        assert_eq!(peak, 0);

        // The playhead's section, by command, in the window's file type.
        d.app().tracks[0].practice_mute = false;
        d.app().export.set_chosen(Some(out.clone()));
        d.call("seek", json!({"bar": 1.5})).unwrap();
        d.command("export-this-section");
        let v = wait(&mut d);
        let last = v["finished"].as_array().unwrap().last().unwrap().clone();
        assert_eq!(last["files"][0]["name"], "01 Verse.wav");
        assert!(out.join("Sections 3").join("01 Verse.wav").exists());
        // Nothing waits, so the queue can't be cleared.
        let e = d
            .call("run_command", json!({"id": "clear-export-queue"}))
            .unwrap_err();
        assert!(e.contains("can't run now"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
