// SPDX-License-Identifier: GPL-3.0-or-later
//! The words sung in a vocal clip, with when each is sung (wish list,
//! song test 3): "Find the words" reads them with a speech model on this
//! computer (crate `daw-speech`, decision 0015), they are drawn along the
//! bottom of each clip of that audio, and assistants read them with
//! `get_lyrics`, to pick a chop by its words.
//!
//! The model isn't shipped with the app. The first time it is needed the
//! app asks, then downloads it once into the user's app data folder and
//! checks its size and SHA-256 before using it.
//!
//! Words belong to the audio file, not the clip: every clip cut from the
//! same recording shows the words in its part of it. They are kept for
//! the session (finding them again takes a few seconds a minute).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};

use daw_engine::time::{Frames, TempoMap};
use daw_speech::{MODELS, Model, Transcript};
use eframe::egui::{self, RichText};
use serde_json::{Value, json};

use crate::App;
use crate::agent::{Access, Tool};
use crate::timeline::Clip;

/// Reads the words in mono samples at the model's rate, with the model
/// file given, saying how far it has got (0 to 100). Tests swap in one
/// that needs no model.
pub(crate) type Engine = fn(&Path, &[f32], Arc<AtomicI32>) -> Result<Transcript, String>;

fn speech_model(
    model: &Path,
    samples: &[f32],
    progress: Arc<AtomicI32>,
) -> Result<Transcript, String> {
    daw_speech::transcribe(model, samples, move |p| {
        progress.store(p, Ordering::Relaxed)
    })
}

/// Where downloaded models are kept: the user's app data folder, e.g.
/// `C:\Users\<name>\AppData\Local\daw\models`.
fn models_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
        .unwrap_or_else(std::env::temp_dir)
        .join("daw")
        .join("models")
}

/// Finding the words of one recording.
struct Job {
    path: PathBuf,
    progress: Arc<AtomicI32>,
    rx: mpsc::Receiver<Result<Transcript, String>>,
}

/// Downloading a model.
struct Download {
    model: &'static Model,
    got: Arc<AtomicU64>,
    rx: mpsc::Receiver<Result<(), String>>,
}

pub(crate) struct Lyrics {
    /// The model used: the one already downloaded, else the default.
    pub(crate) model: &'static Model,
    /// Where models are downloaded to.
    pub(crate) dir: PathBuf,
    pub(crate) engine: Engine,
    jobs: Vec<Job>,
    download: Option<Download>,
    /// Why finding a recording's words failed.
    failed: HashMap<PathBuf, String>,
    /// Why the last download failed.
    download_error: Option<String>,
    /// Clips to read once the model is in, by id.
    waiting: Vec<u64>,
    /// The window asking to download the model, and showing progress.
    pub(crate) open: bool,
}

impl Default for Lyrics {
    fn default() -> Lyrics {
        let dir = models_dir();
        // A model downloaded before is kept to; the default otherwise.
        let model = (MODELS.iter())
            .find(|m| m.name == daw_speech::DEFAULT_MODEL && present(&dir, m))
            .or_else(|| MODELS.iter().find(|m| present(&dir, m)))
            .or_else(|| daw_speech::model(daw_speech::DEFAULT_MODEL))
            .unwrap_or(&MODELS[0]);
        Lyrics {
            model,
            dir,
            engine: speech_model,
            jobs: Vec::new(),
            download: None,
            failed: HashMap::new(),
            download_error: None,
            waiting: Vec::new(),
            open: false,
        }
    }
}

/// Whether `m` has been downloaded into `dir` (its size is checked here;
/// its hash was checked when it came in).
fn present(dir: &Path, m: &Model) -> bool {
    std::fs::metadata(dir.join(m.file)).is_ok_and(|f| f.len() == m.bytes)
}

impl Lyrics {
    /// The model file to use, if it is on this computer. A file named by
    /// `DAW_SPEECH_MODEL` is used as it is.
    fn model_path(&self) -> Option<PathBuf> {
        if let Some(p) = std::env::var_os("DAW_SPEECH_MODEL").map(PathBuf::from)
            && p.is_file()
        {
            return Some(p);
        }
        present(&self.dir, self.model).then(|| self.dir.join(self.model.file))
    }

    fn reading(&self, path: &Path) -> Option<&Job> {
        self.jobs.iter().find(|j| j.path == path)
    }

    /// The model as assistants and the window describe it.
    fn model_value(&self) -> Value {
        json!({
            "name": self.model.name,
            "megabytes": (self.model.bytes as f64 / 1e6).round(),
            "downloaded": self.model_path().is_some(),
            "downloading": self.download.as_ref().map(|d| {
                (d.got.load(Ordering::Relaxed) as f64 * 100.0 / d.model.bytes as f64).round()
            }),
            "licence": "MIT",
            "from": "huggingface.co",
            "choices": MODELS.iter().map(|m| json!({
                "name": m.name,
                "megabytes": (m.bytes as f64 / 1e6).round(),
                "about": m.about,
            })).collect::<Vec<_>>(),
        })
    }
}

/// Downloads `m` into `dir`: to a part file first, checked against its
/// size and SHA-256 before it takes the model's name. `got` counts the
/// bytes so far. Runs on its own thread.
fn download(m: &'static Model, dir: &Path, got: &AtomicU64) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_connect(Some(std::time::Duration::from_secs(20)))
        // Only the app and its version: nothing about the user.
        .user_agent(format!("{}/{}", crate::APP_NAME, env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let r = agent
        .get(m.url)
        .call()
        .map_err(|e| format!("Couldn't reach the model's page: {e}"))?;
    let mut body = r.into_body().into_reader();
    let part = dir.join(format!("{}.part", m.file));
    let mut out = std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut n_total = 0u64;
    loop {
        let n = body
            .read(&mut buf)
            .map_err(|e| format!("The download stopped: {e}"))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        out.write_all(&buf[..n])
            .map_err(|e| format!("{}: {e}", part.display()))?;
        n_total += n as u64;
        got.store(n_total, Ordering::Relaxed);
        if n_total > m.bytes {
            break;
        }
    }
    out.flush().map_err(|e| e.to_string())?;
    drop(out);
    let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if n_total != m.bytes || hex != m.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err("The downloaded model wasn't the file expected, so it was thrown away.".into());
    }
    std::fs::rename(&part, dir.join(m.file)).map_err(|e| e.to_string())
}

/// A timeline position as a bar number with its fraction: 2.5 is
/// halfway through bar 2, as the tools that take a bar read it.
fn bar_at(map: &TempoMap, frames: i64) -> f64 {
    let t = map.ticks_at(Frames(frames));
    let bar = map.bar_beat(t).bar;
    let from = map.bar_start(bar);
    let len = (map.bar_start(bar + 1).0 - from.0).max(1);
    let x = bar as f64 + (t.0 - from.0) as f64 / len as f64;
    (x * 1000.0).round() / 1000.0
}

/// The words of `t` that `c` plays (by each word's middle), in lines, as
/// an assistant reads them.
fn clip_words(c: &Clip, t: &Transcript, map: &TempoMap, rate: u32) -> (Vec<Value>, String) {
    let src = c.source.rate.max(1) as f64;
    let r3 = |x: f64| (x * 1000.0).round() / 1000.0;
    let (lo, hi) = (c.offset as f64, (c.offset + c.len) as f64);
    let mut lines = Vec::new();
    let mut all = Vec::new();
    for l in &t.lines {
        let words: Vec<Value> = (l.words.iter())
            .filter(|w| (lo..hi).contains(&((w.start + w.end) / 2.0 * src)))
            .map(|w| {
                let (a, b) = ((w.start * src).max(lo), (w.end * src).min(hi));
                let out = c.out_of_frame(map, a);
                all.push(w.text.clone());
                json!({
                    "text": w.text,
                    "start_seconds": r3((a - lo) / src),
                    "seconds": r3((b - a) / src),
                    "file_seconds": r3(a / src),
                    "song_seconds": r3(out as f64 / rate.max(1) as f64),
                    "bar": bar_at(map, out),
                    "sure": (w.sure as f64 * 100.0).round() / 100.0,
                })
            })
            .collect();
        if words.is_empty() {
            continue;
        }
        let text = if words.len() == l.words.len() {
            l.text.clone()
        } else {
            (words.iter())
                .filter_map(|w| w["text"].as_str())
                .collect::<Vec<_>>()
                .join(" ")
        };
        lines.push(json!({"text": text, "words": words}));
    }
    (lines, all.join(" "))
}

pub(crate) const TOOLS: &[Tool] = &[
    Tool {
        name: "find_lyrics",
        description: "Starts reading the words sung or spoken in an audio clip's \
                      recording, with a speech model on this computer, and answers \
                      at once with where it has got (as get_lyrics does); call \
                      get_lyrics a few seconds later for the words, which take a few \
                      seconds a minute of audio. Every clip of the same recording \
                      shares them. The model isn't shipped with the app: if it isn't \
                      downloaded yet this fails, saying its size, unless \
                      download_model is true, which downloads it once (from \
                      huggingface.co, MIT licence) and then reads the words. model \
                      picks which (see get_lyrics' model.choices). again: true reads \
                      them again. Changes nothing in the project.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "required": ["clip"], "properties": {
                "clip": {"type": "string", "description": "An audio clip's id."},
                "download_model": {"type": "boolean"},
                "model": {"type": "string", "enum": ["base", "small", "medium"]},
                "again": {"type": "boolean"}
            }})
        },
    },
    Tool {
        name: "get_lyrics",
        description: "The words found in an audio clip (find_lyrics finds them), the \
                      part of its recording the clip plays: status (none, \
                      downloading, reading, found or failed, with progress_pct and \
                      error), the language heard, text (all its words), and lines, \
                      each with its text and words. Each word: text, start_seconds \
                      and seconds (from the clip's start, in its own audio), \
                      file_seconds (where it starts in the \
                      file: edit_clip offset_seconds, to cut a chop of it), \
                      song_seconds and bar (where it plays in the song), and sure (0 \
                      to 1, how sure the model is). Speech models mishear some sung \
                      words, so check by ear before cutting. model says which speech \
                      model is used and whether it is downloaded.",
        access: Access::Read,
        schema: || {
            json!({"type": "object", "required": ["clip"], "properties": {
                "clip": {"type": "string", "description": "An audio clip's id."}
            }})
        },
    },
];

/// Why words can't be found yet.
enum Blocked {
    /// The model isn't on this computer.
    NoModel,
    Other(String),
}

impl App {
    /// Answers one of the looking tools here, or `None` if `name` isn't one.
    pub(crate) fn lyrics_read(
        &mut self,
        name: &str,
        args: &Value,
    ) -> Option<Result<Value, String>> {
        (name == "get_lyrics").then(|| {
            self.lyrics_poll();
            let id = self.lyrics_clip(args)?;
            Ok(self.lyrics_value(id))
        })
    }

    /// Runs one of the tools here that start work, or `None`.
    pub(crate) fn lyrics_tool(
        &mut self,
        name: &str,
        args: &Value,
    ) -> Option<Result<(Value, Option<String>), String>> {
        (name == "find_lyrics").then(|| {
            self.lyrics_poll();
            let id = self.lyrics_clip(args)?;
            if let Some(m) = args["model"].as_str() {
                let m = daw_speech::model(m).ok_or_else(|| {
                    format!(
                        "There is no speech model {m}; there are {}.",
                        MODELS.iter().map(|m| m.name).collect::<Vec<_>>().join(", ")
                    )
                })?;
                if self.lyrics.download.is_some() && m != self.lyrics.model {
                    return Err("Another speech model is downloading.".into());
                }
                self.lyrics.model = m;
            }
            let again = args["again"].as_bool() == Some(true);
            match self.start_lyrics(id, again) {
                Ok(()) => {}
                Err(Blocked::NoModel) if args["download_model"].as_bool() == Some(true) => {
                    self.download_model(id);
                }
                Err(Blocked::NoModel) => {
                    let m = self.lyrics.model;
                    return Err(format!(
                        "The speech model ({}, {:.0} MB, MIT licence) isn't on this \
                         computer yet. Call again with download_model: true to download \
                         it once from huggingface.co (ask the person first if they may \
                         not want that), or pick a smaller one with model.",
                        m.name,
                        m.bytes as f64 / 1e6
                    ));
                }
                Err(Blocked::Other(e)) => return Err(e),
            }
            Ok((self.lyrics_value(id), None))
        })
    }

    /// The clip named by `args`, by its id in the app.
    fn lyrics_clip(&self, args: &Value) -> Result<u64, String> {
        let key = args["clip"].as_str().ok_or("Give the clip's id.")?;
        if let Some(c) = (self.tracks.iter().flat_map(|t| &t.clips)).find(|c| c.key == key) {
            return Ok(c.id);
        }
        if (self.tracks.iter().flat_map(|t| &t.note_clips)).any(|c| c.id == key) {
            return Err(format!(
                "{key} is a note clip; words are read from audio clips."
            ));
        }
        Err(format!("There is no clip {key}."))
    }

    fn clip_by_id(&self, id: u64) -> Option<&Clip> {
        (self.tracks.iter().flat_map(|t| &t.clips)).find(|c| c.id == id)
    }

    /// What is known of clip `id`'s words.
    fn lyrics_value(&self, id: u64) -> Value {
        let Some(c) = self.clip_by_id(id) else {
            return json!({"status": "none"});
        };
        let l = &self.lyrics;
        let mut v = json!({"clip": c.key, "model": l.model_value()});
        if let Some(t) = self.timeline.words.get(&c.path) {
            let (lines, text) = clip_words(c, t, &self.tempo_map(), self.rate);
            v["status"] = json!("found");
            v["language"] = json!(t.language);
            v["text"] = json!(text);
            v["lines"] = json!(lines);
        } else if let Some(j) = l.reading(&c.path) {
            v["status"] = json!("reading");
            v["progress_pct"] = json!(j.progress.load(Ordering::Relaxed).clamp(0, 100));
        } else if l.waiting.contains(&id) && l.download.is_some() {
            v["status"] = json!("downloading");
            v["progress_pct"] = v["model"]["downloading"].clone();
        } else if let Some(e) = l.failed.get(&c.path) {
            v["status"] = json!("failed");
            v["error"] = json!(e);
        } else if let (true, Some(e)) = (l.waiting.contains(&id), &l.download_error) {
            v["status"] = json!("failed");
            v["error"] = json!(e);
        } else {
            v["status"] = json!("none");
        }
        v
    }

    /// Starts finding the words of clip `id`'s recording, unless they are
    /// known (and not `again`) or being found.
    fn start_lyrics(&mut self, id: u64, again: bool) -> Result<(), Blocked> {
        let c = self
            .clip_by_id(id)
            .ok_or_else(|| Blocked::Other("There is no such clip.".into()))?;
        let path = c.path.clone();
        if (self.timeline.words.contains_key(&path) && !again)
            || self.lyrics.reading(&path).is_some()
        {
            return Ok(());
        }
        let Some(model) = self.lyrics.model_path() else {
            return Err(Blocked::NoModel);
        };
        let source = c.source.clone();
        self.lyrics.failed.remove(&path);
        let progress = Arc::new(AtomicI32::new(0));
        let (tx, rx) = mpsc::channel();
        let engine = self.lyrics.engine;
        let p = progress.clone();
        std::thread::spawn(move || {
            let r = source.read_mono(source.frames).and_then(|x| {
                let x = daw_engine::import::resample(&x, 1, source.rate, daw_speech::RATE)?;
                engine(&model, &x, p)
            });
            let _ = tx.send(r);
        });
        self.lyrics.jobs.push(Job { path, progress, rx });
        Ok(())
    }

    /// Downloads the model, then finds clip `id`'s words.
    fn download_model(&mut self, id: u64) {
        if !self.lyrics.waiting.contains(&id) {
            self.lyrics.waiting.push(id);
        }
        if self.lyrics.download.is_some() {
            return;
        }
        self.lyrics.download_error = None;
        let m = self.lyrics.model;
        let dir = self.lyrics.dir.clone();
        let got = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::channel();
        let g = got.clone();
        std::thread::spawn(move || {
            let _ = tx.send(download(m, &dir, &g));
        });
        self.lyrics.download = Some(Download { model: m, got, rx });
    }

    /// Takes in finished downloads and readings.
    fn lyrics_poll(&mut self) {
        if let Some(d) = &self.lyrics.download {
            match d.rx.try_recv() {
                Ok(r) => {
                    self.lyrics.download = None;
                    match r {
                        Ok(()) => {
                            for id in std::mem::take(&mut self.lyrics.waiting) {
                                if let Err(Blocked::Other(e)) = self.start_lyrics(id, false) {
                                    self.message = Some(e);
                                }
                            }
                        }
                        Err(e) => self.lyrics.download_error = Some(e),
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => self.lyrics.download = None,
            }
        }
        let mut done = Vec::new();
        self.lyrics.jobs.retain(|j| match j.rx.try_recv() {
            Ok(r) => {
                done.push((j.path.clone(), r));
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => false,
        });
        for (path, r) in done {
            match r {
                Ok(t) => {
                    let n = t.words().count();
                    self.message = Some(if n == 0 {
                        "No words were heard in that clip.".into()
                    } else {
                        format!("Found {n} words.")
                    });
                    self.timeline.words.insert(path, Arc::new(t));
                }
                Err(e) => {
                    self.message = Some(format!("The words couldn't be read: {e}"));
                    self.lyrics.failed.insert(path, e);
                }
            }
        }
    }

    /// "Find the words": for the selected audio clip.
    pub(crate) fn find_lyrics_command(&mut self) {
        let Some((l, k)) = self.pitch_clip() else {
            self.message = Some("Select an audio clip to find its words.".into());
            return;
        };
        let id = self.tracks[l].clips[k].id;
        match self.start_lyrics(id, true) {
            Ok(()) => self.message = Some("Finding the words…".into()),
            Err(Blocked::NoModel) => {
                if !self.lyrics.waiting.contains(&id) {
                    self.lyrics.waiting.push(id);
                }
                self.lyrics.open = true;
            }
            Err(Blocked::Other(e)) => self.message = Some(e),
        }
    }

    /// Each frame: takes in finished work, and draws the download window.
    pub(crate) fn lyrics_tick(&mut self, ctx: &egui::Context) {
        self.lyrics_poll();
        if self.lyrics.download.is_some() || !self.lyrics.jobs.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        if !self.lyrics.open {
            return;
        }
        let mut open = true;
        let mut start = false;
        let mut close = false;
        let l = &mut self.lyrics;
        let have = l.model_path().is_some();
        egui::Window::new("Speech model")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(
                    "Finding the words in a recording uses a speech model that runs on \
                     this computer. It isn't part of the app: it is downloaded once, \
                     from huggingface.co, and kept in your app data folder. Its licence \
                     is MIT.",
                );
                ui.add_space(4.0);
                let busy = l.download.is_some();
                for m in MODELS {
                    let text = format!("{}: {:.0} MB, {}", m.name, m.bytes as f64 / 1e6, m.about);
                    // ai: find_lyrics
                    if ui
                        .add_enabled(!busy, egui::RadioButton::new(l.model == m, text))
                        .clicked()
                    {
                        l.model = m;
                    }
                }
                ui.add_space(4.0);
                if let Some(d) = &l.download {
                    let k = d.got.load(Ordering::Relaxed) as f32 / d.model.bytes as f32;
                    ui.add(egui::ProgressBar::new(k).show_percentage().text(format!(
                        "Downloading {}: {:.0} of {:.0} MB",
                        d.model.name,
                        d.got.load(Ordering::Relaxed) as f64 / 1e6,
                        d.model.bytes as f64 / 1e6
                    )));
                } else if have {
                    ui.label(RichText::new("Downloaded. Finding the words…").weak());
                } else {
                    if let Some(e) = &l.download_error {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                    ui.horizontal(|ui| {
                        // ai: find_lyrics
                        start = ui.button("Download and find the words").clicked();
                        // ai: view
                        close = ui.button("Not now").clicked();
                    });
                }
                for j in &l.jobs {
                    let name = j
                        .path
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                    ui.add(
                        egui::ProgressBar::new(j.progress.load(Ordering::Relaxed) as f32 / 100.0)
                            .text(format!("Finding the words of {name}")),
                    );
                }
            });
        if start && let Some(&id) = self.lyrics.waiting.first() {
            self.download_model(id);
        }
        if close {
            self.lyrics.waiting.clear();
        }
        if !open
            || close
            || (have && self.lyrics.jobs.is_empty() && self.lyrics.download.is_none() && !start)
        {
            self.lyrics.open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Track;
    use crate::agent::driver::Driver;
    use daw_speech::{Line, Word};

    /// Hears the same three words whatever it is given, a second apart,
    /// over a second of each half of the audio.
    fn fake(_: &Path, x: &[f32], p: Arc<AtomicI32>) -> Result<Transcript, String> {
        p.store(100, Ordering::Relaxed);
        let secs = x.len() as f64 / daw_speech::RATE as f64;
        assert!(
            (secs - 4.0).abs() < 0.01,
            "given at the model's rate: {secs}"
        );
        let w = |t: &str, s: f64| Word {
            text: t.into(),
            start: s,
            end: s + 0.5,
            sure: 0.9,
        };
        Ok(Transcript {
            language: "en".into(),
            lines: vec![
                Line {
                    text: "There is a profit,".into(),
                    words: vec![w("There", 0.5), w("is", 1.0), w("a", 1.5), w("profit", 2.0)],
                },
                Line {
                    text: "somewhere.".into(),
                    words: vec![w("somewhere", 3.0)],
                },
            ],
        })
    }

    /// Four seconds of quiet audio at 48 kHz, as a clip at bar 2 of a
    /// song at 120 bpm (2 s a bar).
    fn vocal(d: &mut Driver) -> String {
        let dir = std::env::temp_dir().join(format!("daw-lyrics-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("vocal.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..4 * 48_000 {
            w.write_sample(0.0f32).unwrap();
        }
        w.finalize().unwrap();
        let src = daw_engine::stream::StreamSource::open(&path).unwrap();
        let app = d.app();
        let start = 2 * app.rate as i64;
        let c = Clip::new(src, path, start, None);
        let key = c.key.clone();
        app.tracks.push(Track::new("Vocal".into()));
        app.tracks[0].clips.push(c);
        app.rebuild();
        key
    }

    /// Calls get_lyrics until the words are in.
    fn found(d: &mut Driver, key: &str) -> Value {
        for _ in 0..500 {
            let v = d.call("get_lyrics", json!({"clip": key})).unwrap();
            if v["status"] != "reading" {
                return v;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("still reading");
    }

    /// Points the app at an empty models folder of its own, and the fake.
    fn no_model(d: &mut Driver, name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("daw-lyrics-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let app = d.app();
        app.lyrics.dir = dir.clone();
        app.lyrics.engine = fake;
        dir
    }

    /// A file the size of the model in `dir`, as after a download.
    fn model_in(d: &mut Driver, dir: &Path) {
        let m = d.app().lyrics.model;
        std::fs::create_dir_all(dir).unwrap();
        std::fs::File::create(dir.join(m.file))
            .unwrap()
            .set_len(m.bytes)
            .unwrap();
    }

    #[test]
    fn an_assistant_reads_a_vocals_words_with_their_times_once_the_model_is_in() {
        let mut d = Driver::new();
        let dir = no_model(&mut d, "read");
        let key = vocal(&mut d);
        let v = d.call("get_lyrics", json!({"clip": key})).unwrap();
        assert_eq!(v["status"], "none");
        assert_eq!(v["model"]["name"], "small");
        assert_eq!(v["model"]["downloaded"], false);
        assert_eq!(v["model"]["megabytes"], 190.0);
        // No model: it says so, with the size, and downloads nothing.
        let e = d.call("find_lyrics", json!({"clip": key})).unwrap_err();
        assert!(e.contains("190 MB") && e.contains("download_model"), "{e}");
        assert!(d.app().lyrics.download.is_none());
        assert!(
            d.call("find_lyrics", json!({"clip": key, "model": "huge"}))
                .is_err()
        );
        model_in(&mut d, &dir);
        let v = d.call("find_lyrics", json!({"clip": key})).unwrap();
        assert!(
            ["reading", "found"].contains(&v["status"].as_str().unwrap()),
            "{v}"
        );
        let v = found(&mut d, &key);
        assert_eq!(v["status"], "found", "{v}");
        assert_eq!(v["language"], "en");
        assert_eq!(v["text"], "There is a profit somewhere");
        assert_eq!(v["lines"][0]["text"], "There is a profit,");
        let w = &v["lines"][0]["words"][3];
        assert_eq!(w["text"], "profit");
        assert_eq!(w["start_seconds"], 2.0);
        assert_eq!(w["seconds"], 0.5);
        assert_eq!(w["file_seconds"], 2.0);
        // The clip starts at 2 s, bar 2; "profit" plays 2 s later, bar 3.
        assert_eq!(w["song_seconds"], 4.0);
        assert_eq!(w["bar"], 3.0);
        assert_eq!(v["lines"][1]["words"][0]["bar"], 3.5);
        // Reading words is no change to the project: nothing to undo.
        assert_eq!(d.app().project().tracks[0].clips.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_chop_shows_only_the_words_it_plays_from_its_own_start() {
        let mut d = Driver::new();
        let dir = no_model(&mut d, "chop");
        model_in(&mut d, &dir);
        let key = vocal(&mut d);
        d.call("find_lyrics", json!({"clip": key})).unwrap();
        assert_eq!(found(&mut d, &key)["status"], "found");
        // The clip cut down to 2 s..3.5 s of its file.
        d.call(
            "edit_clip",
            json!({"clip": key, "offset_seconds": 2.0, "len_seconds": 1.5}),
        )
        .unwrap();
        let v = found(&mut d, &key);
        assert_eq!(v["text"], "profit somewhere", "{v}");
        assert_eq!(v["lines"][0]["text"], "profit", "a part of a line");
        let w = &v["lines"][0]["words"][0];
        assert_eq!(w["start_seconds"], 0.0);
        assert_eq!(w["file_seconds"], 2.0);
        // "somewhere" runs past the clip's end: cut where the clip ends.
        let w = &v["lines"][1]["words"][0];
        assert_eq!(w["start_seconds"], 1.0);
        assert_eq!(w["seconds"], 0.5);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_command_asks_to_download_the_model_first() {
        let mut d = Driver::new();
        let dir = no_model(&mut d, "ask");
        let key = vocal(&mut d);
        let id = d.app().tracks[0].clips[0].id;
        d.app().timeline.selected.insert(id);
        d.command("find-lyrics");
        assert!(d.app().lyrics.open, "the download window");
        assert_eq!(d.app().lyrics.waiting, [id]);
        assert_eq!(
            d.call("get_lyrics", json!({"clip": key})).unwrap()["status"],
            "none"
        );
        // With the model there, the command finds them straight away.
        model_in(&mut d, &dir);
        d.app().lyrics.open = false;
        d.command("find-lyrics");
        assert!(!d.app().lyrics.open);
        assert_eq!(found(&mut d, &key)["status"], "found");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// With a real model and a vocal recording named by
    /// `DAW_SPEECH_MODEL` and `DAW_SPEECH_VOCAL` (a WAV file), reads its
    /// words through the assistant tools and prints them. Skipped
    /// otherwise: CI has no model.
    #[test]
    fn reads_a_real_vocal_through_the_tools_when_given_one() {
        let (Some(_), Some(wav)) = (
            std::env::var_os("DAW_SPEECH_MODEL"),
            std::env::var_os("DAW_SPEECH_VOCAL"),
        ) else {
            return;
        };
        let mut d = Driver::new();
        let path = PathBuf::from(wav);
        let src = daw_engine::stream::StreamSource::open(&path).unwrap();
        let app = d.app();
        let c = Clip::new(src, path, 0, None);
        let key = c.key.clone();
        app.tracks.push(Track::new("Vocal".into()));
        app.tracks[0].clips.push(c);
        app.rebuild();
        d.call("find_lyrics", json!({"clip": key})).unwrap();
        let mut v = Value::Null;
        for _ in 0..1200 {
            v = d.call("get_lyrics", json!({"clip": key})).unwrap();
            if v["status"] != "reading" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        println!("{}", serde_json::to_string_pretty(&v).unwrap());
        assert_eq!(v["status"], "found");
    }

    /// Downloads the smallest model for real when `DAW_SPEECH_DOWNLOAD`
    /// is set, and checks it; skipped otherwise.
    #[test]
    fn downloads_and_checks_a_model_when_asked_to() {
        if std::env::var_os("DAW_SPEECH_DOWNLOAD").is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("daw-lyrics-dl-{}", std::process::id()));
        let got = AtomicU64::new(0);
        download(&MODELS[0], &dir, &got).unwrap();
        assert!(present(&dir, &MODELS[0]));
        assert_eq!(
            daw_speech::verify(&dir.join(MODELS[0].file), &MODELS[0]),
            Ok(true)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
