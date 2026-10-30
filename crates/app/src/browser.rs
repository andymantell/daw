// SPDX-License-Identifier: GPL-3.0-or-later
//! The content browser (roadmap 3.6): the user's own sound folders and
//! the project's audio in one list, found by search and tags, heard
//! before use, and added to the project as a new track at the playhead.
//!
//! A preview can play at the project's tempo: the sound's own tempo
//! comes from what the user set, else from its name ("120 bpm"), else is
//! guessed from its length as a whole number of beats. It is then
//! stretched with the practice stretcher (see `daw_engine::preview`),
//! which is still built at risk on the speed and pitch choice (spike
//! 0.6).
//!
//! The folders, tags and tempos belong to this computer, as the folders
//! do, and are kept in `browser.txt` beside the device settings.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use daw_engine::engine::Command;
use daw_engine::import;
use daw_engine::preview::{self, PreviewSound};
use daw_engine::stretch::SPEED_RANGE;
use eframe::egui::{self, RichText};

/// File name endings the browser lists.
const AUDIO: &[&str] = &[
    "wav", "wave", "flac", "mp3", "ogg", "oga", "aif", "aiff", "m4a", "caf",
];
/// How deep and how many files a scan goes, so a folder chosen by
/// mistake (a whole drive) stays quick.
const MAX_DEPTH: usize = 12;
const MAX_FILES: usize = 20_000;
/// A sound at most this long may be a loop, whose tempo can be guessed
/// from its length.
const LOOP_SECONDS: f64 = 40.0;

/// A sound the browser lists.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Entry {
    pub(crate) path: PathBuf,
    /// The file name, without its ending.
    pub(crate) name: String,
    /// Where it is under the folder it was found in, for searching.
    pub(crate) place: String,
    /// Its length, if known without decoding it.
    pub(crate) seconds: Option<f64>,
}

impl Entry {
    pub(crate) fn new(path: PathBuf, place: String) -> Entry {
        let name = path
            .file_stem()
            .map_or(String::new(), |s| s.to_string_lossy().into_owned());
        Entry {
            path,
            name,
            place,
            seconds: None,
        }
    }
}

/// What the user said about a file.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FileInfo {
    pub(crate) tags: Vec<String>,
    pub(crate) bpm: Option<f64>,
}

/// Which list is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Folders,
    Project,
}

/// Where a sound's tempo came from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Tempo {
    Set(f64),
    Named(f64),
    Guessed(f64),
}

impl Tempo {
    pub(crate) fn bpm(self) -> f64 {
        match self {
            Tempo::Set(b) | Tempo::Named(b) | Tempo::Guessed(b) => b,
        }
    }

    fn text(self) -> String {
        match self {
            Tempo::Set(b) => format!("{} bpm", fmt_bpm(b)),
            Tempo::Named(b) => format!("{} bpm (from its name)", fmt_bpm(b)),
            Tempo::Guessed(b) => format!("about {} bpm (from its length)", fmt_bpm(b)),
        }
    }
}

fn fmt_bpm(b: f64) -> String {
    if (b - b.round()).abs() < 0.05 {
        format!("{}", b.round())
    } else {
        format!("{b:.1}")
    }
}

/// A preview made ready, and what was learned on the way.
struct Ready {
    sound: PreviewSound,
    seconds: f64,
    tempo: Option<Tempo>,
    /// The speed it plays at, 1 when not matched to the project.
    speed: f64,
}

/// Something the pane asks the app to do.
pub(crate) enum Action {
    Send(Command),
    /// Add the file to the project as a new track at the playhead.
    Add(PathBuf),
}

pub(crate) struct Browser {
    pub(crate) folders: Vec<PathBuf>,
    pub(crate) info: BTreeMap<PathBuf, FileInfo>,
    path: Option<PathBuf>,
    pub(crate) found: Vec<Entry>,
    scan: Option<mpsc::Receiver<Vec<Entry>>>,
    pub(crate) source: Source,
    pub(crate) query: String,
    pub(crate) tag: Option<String>,
    pub(crate) selected: Option<PathBuf>,
    /// Previews play at the project's tempo.
    pub(crate) sync: bool,
    pub(crate) looped: bool,
    /// Clicking a sound plays it.
    pub(crate) auto_play: bool,
    pub(crate) level_db: f32,
    /// Lengths and tempos learned from previews.
    learned: BTreeMap<PathBuf, (f64, Option<Tempo>)>,
    preparing: Option<(PathBuf, mpsc::Receiver<Result<Ready, String>>)>,
    /// The sound playing, and the speed it plays at.
    pub(crate) playing: Option<(PathBuf, f64)>,
    /// The tags of the selected sound, as typed.
    tags_text: String,
    pub(crate) note: Option<String>,
}

impl Default for Browser {
    fn default() -> Browser {
        Browser {
            folders: Vec::new(),
            info: BTreeMap::new(),
            path: None,
            found: Vec::new(),
            scan: None,
            source: Source::Folders,
            query: String::new(),
            tag: None,
            selected: None,
            sync: true,
            looped: true,
            auto_play: true,
            level_db: -6.0,
            learned: BTreeMap::new(),
            preparing: None,
            playing: None,
            tags_text: String::new(),
            note: None,
        }
    }
}

impl Browser {
    pub(crate) fn new(path: Option<PathBuf>) -> Browser {
        let mut b = Browser::default();
        if let Some(text) = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) {
            b.read(&text);
        }
        b.path = path;
        b.rescan();
        b
    }

    /// `folder <path>`, the preview settings, then `file
    /// <path>\t<bpm or ->\t<tag,tag>` for each file the user described.
    pub(crate) fn to_text(&self) -> String {
        let on = |b: bool| if b { "on" } else { "off" };
        let mut t = String::new();
        for f in &self.folders {
            t.push_str(&format!("folder {}\n", f.display()));
        }
        t.push_str(&format!(
            "sync {}\nloop {}\nauto-play {}\nlevel {}\n",
            on(self.sync),
            on(self.looped),
            on(self.auto_play),
            self.level_db
        ));
        for (p, i) in &self.info {
            if i == &FileInfo::default() {
                continue;
            }
            let bpm = i.bpm.map_or("-".into(), |b| b.to_string());
            t.push_str(&format!(
                "file {}\t{bpm}\t{}\n",
                p.display(),
                i.tags.join(",")
            ));
        }
        t
    }

    pub(crate) fn read(&mut self, text: &str) {
        for line in text.lines() {
            let (what, rest) = line.split_once(' ').unwrap_or((line, ""));
            let on = rest.trim() == "on";
            match what {
                "folder" if !rest.trim().is_empty() => self.folders.push(PathBuf::from(rest)),
                "sync" => self.sync = on,
                "loop" => self.looped = on,
                "auto-play" => self.auto_play = on,
                "level" => {
                    if let Ok(db) = rest.trim().parse::<f32>() {
                        self.level_db = db.clamp(-40.0, 6.0);
                    }
                }
                "file" => {
                    let mut parts = rest.split('\t');
                    let (Some(p), Some(bpm), tags) = (parts.next(), parts.next(), parts.next())
                    else {
                        continue;
                    };
                    let info = FileInfo {
                        tags: tags.map(parse_tags).unwrap_or_default(),
                        bpm: bpm.parse().ok().filter(|b| valid_bpm(*b)),
                    };
                    self.info.insert(PathBuf::from(p), info);
                }
                _ => {}
            }
        }
    }

    fn save(&mut self) {
        let Some(path) = &self.path else { return };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            // Written whole and renamed, so a crash never leaves half a file.
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, self.to_text())?;
            std::fs::rename(tmp, path)
        };
        if let Err(e) = write() {
            self.note = Some(format!("Couldn't save the browser's folders and tags: {e}"));
        }
    }

    pub(crate) fn add_folder(&mut self, dir: PathBuf) {
        let dir = std::path::absolute(&dir).unwrap_or(dir);
        if !self.folders.contains(&dir) {
            self.folders.push(dir);
            self.save();
            self.rescan();
        }
    }

    pub(crate) fn remove_folder(&mut self, n: usize) {
        if n < self.folders.len() {
            self.folders.remove(n);
            self.save();
            self.rescan();
        }
    }

    /// Looks through the folders again, in the background.
    pub(crate) fn rescan(&mut self) {
        let folders = self.folders.clone();
        if folders.is_empty() {
            self.found.clear();
            self.scan = None;
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut all = Vec::new();
            for f in &folders {
                scan(f, &mut all);
            }
            let _ = tx.send(all);
        });
        self.scan = Some(rx);
    }

    pub(crate) fn scanning(&self) -> bool {
        self.scan.is_some()
    }

    pub(crate) fn tags_of(&self, p: &Path) -> &[String] {
        self.info.get(p).map_or(&[], |i| &i.tags)
    }

    pub(crate) fn set_tags(&mut self, p: &Path, tags: Vec<String>) {
        self.info.entry(p.to_path_buf()).or_default().tags = tags;
        self.save();
    }

    pub(crate) fn set_bpm(&mut self, p: &Path, bpm: Option<f64>) {
        self.info.entry(p.to_path_buf()).or_default().bpm = bpm.filter(|b| valid_bpm(*b));
        self.save();
    }

    /// Every tag used, in order.
    pub(crate) fn all_tags(&self) -> Vec<String> {
        let mut t: Vec<String> = self.info.values().flat_map(|i| i.tags.clone()).collect();
        t.sort();
        t.dedup();
        t
    }

    /// The sound's tempo, as far as is known without hearing it.
    pub(crate) fn tempo(&self, e: &Entry, project_bpm: f64) -> Option<Tempo> {
        if let Some(b) = self.info.get(&e.path).and_then(|i| i.bpm) {
            return Some(Tempo::Set(b));
        }
        if let Some(b) = bpm_in_name(&e.name) {
            return Some(Tempo::Named(b));
        }
        let secs = e
            .seconds
            .or_else(|| self.learned.get(&e.path).map(|l| l.0))?;
        guess_bpm(secs, project_bpm).map(Tempo::Guessed)
    }

    fn seconds(&self, e: &Entry) -> Option<f64> {
        e.seconds.or_else(|| self.learned.get(&e.path).map(|l| l.0))
    }

    /// The entries of `list` that the search and tag let through.
    pub(crate) fn shown<'a>(&self, list: &'a [Entry]) -> Vec<&'a Entry> {
        let words: Vec<String> = self
            .query
            .split_whitespace()
            .map(|w| w.to_lowercase())
            .collect();
        list.iter()
            .filter(|e| {
                let tags = self.tags_of(&e.path);
                if let Some(t) = &self.tag
                    && !tags.contains(t)
                {
                    return false;
                }
                let name = e.name.to_lowercase();
                let place = e.place.to_lowercase();
                words.iter().all(|w| {
                    name.contains(w.as_str())
                        || place.contains(w.as_str())
                        || tags.iter().any(|t| t.to_lowercase().contains(w.as_str()))
                })
            })
            .collect()
    }

    /// Takes in a finished scan and a finished preview. Call every frame.
    pub(crate) fn update(&mut self) -> Vec<Action> {
        if let Some(rx) = &self.scan {
            match rx.try_recv() {
                Ok(found) => {
                    self.found = found;
                    self.scan = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => self.scan = None,
            }
        }
        let mut out = Vec::new();
        let Some((path, rx)) = &self.preparing else {
            return out;
        };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return out,
            Err(mpsc::TryRecvError::Disconnected) => Err("the preview stopped".into()),
        };
        let path = path.clone();
        self.preparing = None;
        match result {
            Ok(r) => {
                self.learned.insert(path.clone(), (r.seconds, r.tempo));
                self.playing = Some((path, r.speed));
                out.push(Action::Send(Command::Preview(Some(Box::new(r.sound)))));
            }
            Err(e) => {
                self.note = Some(format!("Couldn't play it: {e}"));
                self.playing = None;
            }
        }
        out
    }

    pub(crate) fn busy(&self) -> bool {
        self.preparing.is_some() || self.scan.is_some()
    }

    /// Starts making a preview of `e` for a device at `rate`.
    pub(crate) fn play(&mut self, e: &Entry, rate: u32, project_bpm: f64) {
        let path = e.path.clone();
        let set = self.info.get(&path).and_then(|i| i.bpm);
        let name = e.name.clone();
        let (sync, looped) = (self.sync, self.looped);
        let gain = db_to_gain(self.level_db);
        let (tx, rx) = mpsc::channel();
        let p = path.clone();
        std::thread::spawn(move || {
            let make = || -> Result<Ready, String> {
                let s = import::read(&p)?;
                let seconds = s.samples.len() as f64 / s.channels.max(1) as f64 / s.rate as f64;
                let tempo = set
                    .map(Tempo::Set)
                    .or_else(|| bpm_in_name(&name).map(Tempo::Named))
                    .or_else(|| guess_bpm(seconds, project_bpm).map(Tempo::Guessed));
                let speed = match tempo {
                    Some(t) if sync => fit_speed(project_bpm, t.bpm()).unwrap_or(1.0),
                    _ => 1.0,
                };
                let mut sound = preview::prepare(&s, rate, speed)?;
                sound.gain = gain;
                sound.looped = looped;
                Ok(Ready {
                    sound,
                    seconds,
                    tempo,
                    speed,
                })
            };
            let _ = tx.send(make());
        });
        self.note = None;
        self.preparing = Some((path, rx));
    }

    /// Stops the preview. Returns the command to send.
    pub(crate) fn stop(&mut self) -> Command {
        self.preparing = None;
        self.playing = None;
        Command::Preview(None)
    }

    /// True while `p` is playing or being made ready.
    pub(crate) fn is_playing(&self, p: &Path) -> bool {
        self.playing.as_ref().is_some_and(|(q, _)| q == p)
            || self.preparing.as_ref().is_some_and(|(q, _)| q == p)
    }

    /// Notices the engine has finished a preview that doesn't loop.
    pub(crate) fn engine_stopped(&mut self) {
        if self.preparing.is_none() {
            self.playing = None;
        }
    }

    /// Draws the pane. `project` lists the project's audio; `progress`
    /// is where the preview voice is, in seconds, and how long.
    pub(crate) fn pane(
        &mut self,
        ui: &mut egui::Ui,
        project: &[Entry],
        rate: u32,
        project_bpm: f64,
        progress: Option<(f64, f64)>,
    ) -> Vec<Action> {
        let mut out = Vec::new();
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.source, Source::Folders, "Sound folders")
                .on_hover_text("Sounds in folders you choose on this computer");
            ui.selectable_value(&mut self.source, Source::Project, "This project")
                .on_hover_text("Audio the project already uses");
            ui.separator();
            ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("Search names and tags")
                    .desired_width(ui.available_width().clamp(80.0, 220.0)),
            )
            .accessible_name("Search sounds");
            if !self.query.is_empty()
                && ui
                    .small_button("✕")
                    .accessible_name("Clear the search")
                    .on_hover_text("Clear")
                    .clicked()
            {
                self.query.clear();
            }
        });
        if self.source == Source::Folders {
            self.folders_row(ui);
        }
        let tags = self.all_tags();
        if !tags.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.weak("Tags");
                for t in tags {
                    let on = self.tag.as_ref() == Some(&t);
                    if ui
                        .selectable_label(on, format!("#{t}"))
                        .on_hover_text(if on { "Show all" } else { "Show only these" })
                        .clicked()
                    {
                        self.tag = if on { None } else { Some(t) };
                    }
                }
            });
        }
        ui.separator();
        let list: Vec<Entry> = match self.source {
            Source::Folders => self.shown(&self.found).into_iter().cloned().collect(),
            Source::Project => self.shown(project).into_iter().cloned().collect(),
        };
        let details = 150.0;
        let list_height = (ui.available_height() - details).max(80.0);
        if list.is_empty() {
            ui.allocate_ui(egui::vec2(ui.available_width(), list_height), |ui| {
                ui.weak(match self.source {
                    Source::Folders if self.folders.is_empty() => {
                        "Add a folder of sounds to browse it here."
                    }
                    Source::Folders if self.scanning() => "Looking through the folders…",
                    Source::Project if project.is_empty() => "The project has no audio yet.",
                    _ => "Nothing matches.",
                });
            });
        } else {
            egui::ScrollArea::vertical()
                .id_salt("browser-list")
                .max_height(list_height)
                .auto_shrink([false, true])
                .show_rows(ui, 22.0, list.len(), |ui, rows| {
                    for e in &list[rows] {
                        self.row(ui, e, rate, project_bpm, &mut out);
                    }
                });
        }
        ui.separator();
        self.preview_row(ui, progress, &mut out);
        let sel = self
            .selected
            .clone()
            .and_then(|p| list.iter().find(|e| e.path == p).cloned());
        if let Some(e) = sel {
            self.details(ui, &e, project_bpm, &mut out);
        }
        if let Some(n) = &self.note {
            ui.colored_label(ui.visuals().warn_fg_color, n);
        }
        out
    }

    fn folders_row(&mut self, ui: &mut egui::Ui) {
        let mut remove = None;
        ui.horizontal_wrapped(|ui| {
            for (n, f) in self.folders.iter().enumerate() {
                let name = f.file_name().map_or(f.display().to_string(), |s| {
                    s.to_string_lossy().into_owned()
                });
                ui.label(RichText::new(format!("🗁 {name}")))
                    .on_hover_text(f.display().to_string());
                if ui
                    .small_button("✕")
                    .accessible_name(format!("Stop browsing {name}"))
                    .on_hover_text("Stop browsing this folder (its files stay where they are)")
                    .clicked()
                {
                    remove = Some(n);
                }
            }
            if ui.button("Add folder…").clicked()
                && let Some(dir) = rfd::FileDialog::new()
                    .set_title("Add a folder of sounds")
                    .pick_folder()
            {
                self.add_folder(dir);
            }
            if !self.folders.is_empty()
                && ui
                    .add_enabled(!self.scanning(), egui::Button::new("Look again"))
                    .on_hover_text("Find files added to the folders since")
                    .clicked()
            {
                self.rescan();
            }
            if self.scanning() {
                ui.spinner();
            }
        });
        if let Some(n) = remove {
            self.remove_folder(n);
        }
    }

    fn row(
        &mut self,
        ui: &mut egui::Ui,
        e: &Entry,
        rate: u32,
        project_bpm: f64,
        out: &mut Vec<Action>,
    ) {
        ui.horizontal(|ui| {
            let playing = self.is_playing(&e.path);
            let r = ui
                .small_button(if playing { "■" } else { "▶" })
                .accessible_name(if playing {
                    format!("Stop {}", e.name)
                } else {
                    format!("Play {}", e.name)
                })
                .on_hover_text(if playing { "Stop" } else { "Hear it" });
            if r.clicked() {
                self.selected = Some(e.path.clone());
                self.tags_text = self.tags_of(&e.path).join(", ");
                if playing {
                    out.push(Action::Send(self.stop()));
                } else {
                    self.play(e, rate, project_bpm);
                }
            }
            let chosen = self.selected.as_ref() == Some(&e.path);
            let r = ui
                .selectable_label(chosen, &e.name)
                .on_hover_text(e.path.display().to_string());
            if r.clicked() {
                self.selected = Some(e.path.clone());
                self.tags_text = self.tags_of(&e.path).join(", ");
                if self.auto_play {
                    self.play(e, rate, project_bpm);
                }
            }
            if r.double_clicked() {
                out.push(Action::Add(e.path.clone()));
            }
            r.dnd_set_drag_payload(e.path.clone());
            let mut facts = Vec::new();
            match self.tempo(e, project_bpm) {
                Some(Tempo::Guessed(b)) => facts.push(format!("~{} bpm", fmt_bpm(b))),
                Some(t) => facts.push(format!("{} bpm", fmt_bpm(t.bpm()))),
                None => {}
            }
            if let Some(s) = self.seconds(e) {
                facts.push(fmt_secs(s));
            }
            let tags = self.tags_of(&e.path);
            if !tags.is_empty() {
                facts.push(
                    tags.iter()
                        .map(|t| format!("#{t}"))
                        .collect::<Vec<_>>()
                        .join(" "),
                );
            }
            ui.weak(facts.join(" · "));
        });
    }

    fn preview_row(
        &mut self,
        ui: &mut egui::Ui,
        progress: Option<(f64, f64)>,
        out: &mut Vec<Action>,
    ) {
        let before = (self.sync, self.looped, self.auto_play, self.level_db);
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.sync, "At project tempo")
                .on_hover_text("Stretch loops to the project's tempo, keeping their pitch");
            ui.checkbox(&mut self.looped, "Loop");
            ui.checkbox(&mut self.auto_play, "Play on click");
            ui.add(
                egui::Slider::new(&mut self.level_db, -40.0..=6.0)
                    .suffix(" dB")
                    .fixed_decimals(0)
                    .text("Level"),
            )
            .accessible_name("Preview level");
        });
        ui.horizontal(|ui| {
            if let Some((path, speed)) = &self.playing
                && let Some((at, len)) = progress
            {
                let name = path
                    .file_stem()
                    .map_or(String::new(), |s| s.to_string_lossy().into_owned());
                let mut s = format!("Playing {name}, {} of {}", fmt_secs(at), fmt_secs(len));
                if (speed - 1.0).abs() > 1e-6 {
                    s.push_str(&format!(", at {:.0}% speed", speed * 100.0));
                }
                if ui.small_button("■ Stop").clicked() {
                    out.push(Action::Send(self.stop()));
                }
                ui.weak(s);
            } else if self.preparing.is_some() {
                ui.spinner();
                ui.weak("Getting it ready…");
            } else {
                ui.weak("Nothing playing");
            }
        });
        if (self.sync, self.looped, self.auto_play, self.level_db) != before {
            self.save();
        }
    }

    fn details(&mut self, ui: &mut egui::Ui, e: &Entry, project_bpm: f64, out: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            ui.strong(&e.name);
            ui.weak(e.path.display().to_string());
        });
        ui.horizontal(|ui| {
            ui.label("Tempo");
            match self.tempo(e, project_bpm) {
                Some(t) => ui.label(t.text()),
                None => ui.weak("not known"),
            };
            let set = self.info.get(&e.path).and_then(|i| i.bpm);
            let mut bpm = set
                .or(self.tempo(e, project_bpm).map(|t| t.bpm()))
                .unwrap_or(project_bpm);
            let r = ui
                .add(
                    egui::DragValue::new(&mut bpm)
                        .range(20.0..=400.0)
                        .speed(0.1)
                        .fixed_decimals(1)
                        .suffix(" bpm"),
                )
                .accessible_name("Its tempo")
                .on_hover_text("Set its tempo, for playing it at the project's");
            if r.changed() {
                self.set_bpm(&e.path, Some(bpm));
            }
            if set.is_some()
                && ui
                    .small_button("Forget")
                    .on_hover_text("Find its tempo again")
                    .clicked()
            {
                self.set_bpm(&e.path, None);
            }
            if self.sync
                && let Some(t) = self.tempo(e, project_bpm)
                && fit_speed(project_bpm, t.bpm()).is_none()
            {
                ui.weak("too far from the project's tempo: plays as it is");
            }
        });
        ui.horizontal(|ui| {
            ui.label("Tags");
            let r = ui
                .add(
                    egui::TextEdit::singleline(&mut self.tags_text)
                        .hint_text("such as drums, funk")
                        .desired_width(200.0),
                )
                .accessible_name("Its tags");
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if r.lost_focus() || enter {
                let tags = parse_tags(&self.tags_text);
                if tags.as_slice() != self.tags_of(&e.path) {
                    self.set_tags(&e.path, tags);
                }
            }
            if ui
                .button("Add to project")
                .on_hover_text("As a new track, at the playhead (or double-click it)")
                .clicked()
            {
                out.push(Action::Add(e.path.clone()));
            }
        });
    }
}

fn valid_bpm(b: f64) -> bool {
    b.is_finite() && (20.0..=400.0).contains(&b)
}

fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

pub(crate) fn fmt_secs(s: f64) -> String {
    if s < 10.0 {
        format!("{s:.1} s")
    } else {
        let s = s.round() as u64;
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Tags typed as a list: split at commas (or spaces), without `#`,
/// lower case, each once.
pub(crate) fn parse_tags(text: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for t in text.split([',', ' ']) {
        let t = t.trim().trim_start_matches('#').to_lowercase();
        if !t.is_empty() && !tags.contains(&t) {
            tags.push(t);
        }
    }
    tags
}

/// A tempo written in a file name, as "120bpm", "120 BPM", "120_bpm" or
/// "bpm120".
pub(crate) fn bpm_in_name(name: &str) -> Option<f64> {
    let lower = name.to_lowercase();
    let b = lower.as_bytes();
    let mut i = 0;
    while let Some(at) = lower[i..].find("bpm") {
        let at = i + at;
        // The number before it, past one space, '_' or '-'.
        let mut end = at;
        if end > 0 && matches!(b[end - 1], b' ' | b'_' | b'-') {
            end -= 1;
        }
        let start = (0..end)
            .rev()
            .take_while(|&k| b[k].is_ascii_digit() || b[k] == b'.')
            .last();
        if let Some(s) = start
            && let Ok(v) = lower[s..end].parse::<f64>()
            && valid_bpm(v)
        {
            return Some(v);
        }
        // Or the number after it.
        let mut s = at + 3;
        if s < b.len() && matches!(b[s], b' ' | b'_' | b'-') {
            s += 1;
        }
        let e = (s..b.len())
            .take_while(|&k| b[k].is_ascii_digit() || b[k] == b'.')
            .last();
        if let Some(e) = e
            && let Ok(v) = lower[s..=e].parse::<f64>()
            && valid_bpm(v)
        {
            return Some(v);
        }
        i = at + 3;
    }
    None
}

/// A loop's likely tempo from its length: a whole number of beats (1 to
/// 64, a power of two) between 70 and 180 bpm, the one nearest the
/// project's. None for sounds too long to be loops.
pub(crate) fn guess_bpm(seconds: f64, project_bpm: f64) -> Option<f64> {
    if !(0.2..=LOOP_SECONDS).contains(&seconds) {
        return None;
    }
    (0..=6)
        .map(|k| 60.0 * (1u32 << k) as f64 / seconds)
        .filter(|b| (70.0..180.0).contains(b))
        .min_by(|a, b| (a - project_bpm).abs().total_cmp(&(b - project_bpm).abs()))
}

/// The speed that plays a sound at `bpm` in the project's tempo, taking
/// it at double or half time if that stretches it less or brings it
/// within the stretcher's range. None if nothing does.
pub(crate) fn fit_speed(project_bpm: f64, bpm: f64) -> Option<f64> {
    // The one nearest normal speed, which stretches least.
    [1.0, 2.0, 0.5]
        .into_iter()
        .map(|k| project_bpm / (bpm * k))
        .filter(|s| (SPEED_RANGE.0..=SPEED_RANGE.1).contains(s))
        .min_by(|a, b| a.ln().abs().total_cmp(&b.ln().abs()))
}

/// Adds the audio files under `dir` to `all`.
fn scan(dir: &Path, all: &mut Vec<Entry>) {
    let mut stack = vec![(dir.to_path_buf(), 0)];
    let start = all.len();
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        let mut items: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        items.sort();
        for p in items.into_iter().rev() {
            let hidden = p
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if hidden {
                continue;
            }
            if p.is_dir() {
                if depth < MAX_DEPTH {
                    stack.push((p, depth + 1));
                }
            } else if is_audio(&p) {
                if all.len() - start >= MAX_FILES {
                    return;
                }
                let place = p
                    .parent()
                    .and_then(|q| q.strip_prefix(dir.parent().unwrap_or(dir)).ok())
                    .map_or(String::new(), |q| q.display().to_string());
                let mut e = Entry::new(p.clone(), place);
                e.seconds = wav_seconds(&p);
                all.push(e);
            }
        }
    }
    all[start..].sort_by(|a, b| a.path.cmp(&b.path));
}

pub(crate) fn is_audio(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO.contains(&e.to_lowercase().as_str()))
}

/// A WAV file's length from its header.
fn wav_seconds(p: &Path) -> Option<f64> {
    let r = hound::WavReader::open(p).ok()?;
    let rate = r.spec().sample_rate;
    (rate > 0).then(|| r.duration() as f64 / rate as f64)
}

impl crate::App {
    /// The project's audio, each file once, for the browser.
    fn project_sounds(&self) -> Vec<Entry> {
        let mut seen: Vec<Entry> = Vec::new();
        for c in self.tracks.iter().flat_map(|t| &t.clips) {
            if seen.iter().any(|e| e.path == c.path) {
                continue;
            }
            let track = self
                .tracks
                .iter()
                .find(|t| t.clips.iter().any(|d| d.path == c.path))
                .map_or(String::new(), |t| t.name.clone());
            let mut e = Entry::new(c.path.clone(), track);
            e.name = c.label.clone();
            e.seconds = Some(c.source.frames as f64 / c.source.rate.max(1) as f64);
            seen.push(e);
        }
        seen.sort_by_key(|e| e.name.to_lowercase());
        seen
    }

    fn project_bpm(&self) -> f64 {
        let map = self.tempo_map();
        map.tempo_at(map.ticks_at(self.ctl.status().position()))
            .as_f64()
    }

    /// Takes in finished previews and scans. Call every frame.
    pub(crate) fn browser_update(&mut self, ctx: &egui::Context) {
        let progress = self.ctl.status().preview();
        if progress.is_none() && self.browser.playing.is_some() {
            // Finished, unless a new one is on its way to the engine.
            if !self.browser_sent {
                self.browser.engine_stopped();
            }
        } else {
            self.browser_sent = false;
        }
        for a in self.browser.update() {
            self.browser_act(a);
        }
        if self.browser.busy() || self.browser.playing.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn browser_act(&mut self, a: Action) {
        match a {
            Action::Send(c) => {
                if matches!(c, Command::Preview(Some(_))) {
                    self.browser_sent = true;
                }
                self.send(c);
            }
            Action::Add(p) => {
                let at = self.ctl.status().position().0;
                self.add_file_at(&p, at);
            }
        }
    }

    pub(crate) fn browser_pane(&mut self, ui: &mut egui::Ui) {
        let project = self.project_sounds();
        let rate = self.rate;
        let bpm = self.project_bpm();
        let progress = self
            .ctl
            .status()
            .preview()
            .map(|(at, len)| (at as f64 / rate as f64, len as f64 / rate as f64));
        for a in self.browser.pane(ui, &project, rate, bpm, progress) {
            self.browser_act(a);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tempo_from_names() {
        assert_eq!(bpm_in_name("Funk Drums 96bpm"), Some(96.0));
        assert_eq!(bpm_in_name("loop_120_BPM_Am"), Some(120.0));
        assert_eq!(bpm_in_name("bass-BPM-87.5"), Some(87.5));
        assert_eq!(bpm_in_name("hats bpm140"), Some(140.0));
        assert_eq!(bpm_in_name("kick 01"), None);
        // Not a tempo.
        assert_eq!(bpm_in_name("take 5bpm"), None);
    }

    #[test]
    fn guesses_loop_tempo_from_length() {
        // Two bars of 4/4 at 120 bpm: 4 s.
        assert_eq!(guess_bpm(4.0, 120.0), Some(120.0));
        // 4 s is also 4 beats at 60 (too slow) or 16 at 240 (too fast);
        // a project at 90 still gets 120, the only fit.
        assert_eq!(guess_bpm(4.0, 90.0), Some(120.0));
        // 3.2 s: 8 beats at 150 or 4 at 75; nearest the project's.
        assert_eq!(guess_bpm(3.2, 80.0), Some(75.0));
        assert_eq!(guess_bpm(3.2, 140.0), Some(150.0));
        // Too long to be a loop.
        assert_eq!(guess_bpm(95.0, 120.0), None);
    }

    #[test]
    fn fits_speed_with_double_or_half_time() {
        assert_eq!(fit_speed(120.0, 100.0), Some(1.2));
        // 60 bpm at 120 is twice as fast: taken as 120 at half time.
        assert_eq!(fit_speed(120.0, 60.0), Some(1.0));
        assert_eq!(fit_speed(90.0, 170.0), Some(90.0 / 85.0));
        assert_eq!(fit_speed(120.0, 30.0), None);
    }

    #[test]
    fn tags_are_tidied() {
        assert_eq!(
            parse_tags("Drums, #funk  drums,,Loop"),
            ["drums", "funk", "loop"]
        );
    }

    fn entry(path: &str, place: &str) -> Entry {
        Entry::new(PathBuf::from(path), place.into())
    }

    #[test]
    fn search_matches_every_word_in_names_places_and_tags() {
        let mut b = Browser::default();
        let list = [
            entry("/s/Drums/funk kick.wav", "Drums"),
            entry("/s/Drums/rock snare.wav", "Drums"),
            entry("/s/Keys/pad.wav", "Keys"),
        ];
        b.info.insert(
            PathBuf::from("/s/Keys/pad.wav"),
            FileInfo {
                tags: vec!["warm".into()],
                bpm: None,
            },
        );
        let names = |b: &Browser| -> Vec<String> {
            b.shown(&list).iter().map(|e| e.name.clone()).collect()
        };
        assert_eq!(names(&b).len(), 3);
        b.query = "drums KICK".into();
        assert_eq!(names(&b), ["funk kick"]);
        b.query = "warm".into();
        assert_eq!(names(&b), ["pad"]);
        b.query.clear();
        b.tag = Some("warm".into());
        assert_eq!(names(&b), ["pad"]);
    }

    #[test]
    fn keeps_folders_tags_and_tempos() {
        let mut b = Browser::default();
        b.folders.push(PathBuf::from("/home/me/Sounds"));
        b.sync = false;
        b.level_db = -12.0;
        b.info.insert(
            PathBuf::from("/home/me/Sounds/a b.wav"),
            FileInfo {
                tags: vec!["drums".into(), "funk".into()],
                bpm: Some(96.5),
            },
        );
        b.info.insert(
            PathBuf::from("/home/me/Sounds/c.wav"),
            FileInfo {
                tags: vec!["pad".into()],
                bpm: None,
            },
        );
        let mut c = Browser::default();
        c.read(&b.to_text());
        assert_eq!(c.folders, b.folders);
        assert_eq!(c.info, b.info);
        assert!(!c.sync && c.looped);
        assert_eq!(c.level_db, -12.0);
    }

    fn write_wav(path: &Path, rate: u32, frames: usize) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..frames {
            w.write_sample(((i as f32 * 0.05).sin() * 8000.0) as i16)
                .unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn scans_folders_for_audio() {
        let dir = std::env::temp_dir().join(format!("daw-browser-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Drums/Loops")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        write_wav(&dir.join("Drums/Loops/beat 120bpm.wav"), 8000, 16_000);
        write_wav(&dir.join(".hidden/x.wav"), 8000, 10);
        std::fs::write(dir.join("Drums/readme.txt"), "not audio").unwrap();
        std::fs::write(dir.join("Drums/kick.FLAC"), "").unwrap();
        let mut all = Vec::new();
        scan(&dir, &mut all);
        let names: Vec<_> = all.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["beat 120bpm", "kick"]);
        assert_eq!(all[0].seconds, Some(2.0));
        assert!(all[0].place.ends_with("Loops"), "{}", all[0].place);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn previews_at_the_project_tempo() {
        let dir = std::env::temp_dir().join(format!("daw-browser-play-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // One bar at 100 bpm: 2.4 s.
        let p = dir.join("groove 100bpm.wav");
        write_wav(&p, 48_000, 115_200);
        let mut b = Browser::default();
        let e = Entry::new(p.clone(), String::new());
        b.play(&e, 48_000, 120.0);
        let mut got = Vec::new();
        for _ in 0..500 {
            got = b.update();
            if !got.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let [Action::Send(Command::Preview(Some(s)))] = got.as_slice() else {
            panic!("no preview: {:?}", b.note);
        };
        // Sped up by 1.2: 2 s, one bar at 120.
        assert_eq!(s.len(), 96_000);
        assert!(s.looped);
        assert_eq!(b.playing, Some((p.clone(), 1.2)));
        assert_eq!(b.tempo(&e, 120.0), Some(Tempo::Named(100.0)));
        assert!(matches!(b.stop(), Command::Preview(None)));
        assert!(!b.is_playing(&p));
        let _ = std::fs::remove_dir_all(&dir);
    }

    mod ui {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable as _;

        use super::*;
        use crate::App;
        use crate::commands::Cmd;
        use crate::workspace::Pane;

        fn wait_for(h: &mut Harness<'_, App>, what: impl Fn(&App) -> bool) -> bool {
            for _ in 0..300 {
                h.step();
                if what(h.state()) {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            false
        }

        #[test]
        fn finds_hears_and_adds_a_sound_at_the_playhead() {
            let dir = std::env::temp_dir().join(format!("daw-browser-ui-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("Loops")).unwrap();
            write_wav(&dir.join("Loops/groove 100bpm.wav"), 48_000, 115_200);
            write_wav(&dir.join("Loops/pad.wav"), 48_000, 48_000);
            let mut h = Harness::builder()
                .with_size([1400.0, 900.0])
                .allow_missing_glyphs()
                .build_eframe(|_cc| App::new(Vec::new()));
            let ctx = h.ctx.clone();
            h.state_mut().run(&ctx, Cmd::Browser);
            h.run();
            assert!(h.state().workspace.layout.docked(Pane::Browser));
            h.state_mut().browser.add_folder(dir.clone());
            assert!(wait_for(&mut h, |a| a.browser.found.len() == 2));
            h.run();
            assert!(h.query_by_label("Play pad").is_some());
            // Search narrows the list.
            h.state_mut().browser.query = "groove".into();
            h.run();
            assert!(h.query_by_label("Play pad").is_none());
            h.get_by_label("Play groove 100bpm").click();
            assert!(wait_for(&mut h, |a| a.ctl.status().preview().is_some()));
            let (_, len) = h.state().ctl.status().preview().unwrap();
            // At the project's 120 bpm: one bar in 2 s.
            assert_eq!(len, 96_000);
            h.run();
            h.get_by_label("Stop groove 100bpm").click();
            assert!(wait_for(&mut h, |a| a.ctl.status().preview().is_none()));
            // Added as a track, its clip at the playhead.
            h.state_mut()
                .send(Command::Seek(daw_engine::time::Frames(24_000)));
            assert!(wait_for(&mut h, |a| a.ctl.status().position().0 == 24_000));
            h.get_by_label("Add to project").click();
            assert!(wait_for(&mut h, |a| a.tracks.len() == 1));
            let t = &h.state().tracks[0];
            assert_eq!(t.name, "groove 100bpm");
            assert_eq!(t.clips[0].start, 24_000);
            // And listed under the project's audio.
            h.state_mut().browser.source = Source::Project;
            h.state_mut().browser.query.clear();
            h.run();
            assert!(h.query_by_label("Play groove 100bpm").is_some());
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
