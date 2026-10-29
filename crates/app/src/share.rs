// SPDX-License-Identifier: GPL-3.0-or-later
//! Sharing a project without a server (roadmap 1.6): bundles to send by
//! any means, and shared-folder sync.
//!
//! Each computer writes its own copy of the document into the project's
//! `Devices` folder soon after each edit, and watches that folder for
//! other computers' copies, which are merged in as they arrive. So a
//! project folder in a synced folder stays in step on every computer
//! that has it open, at the speed the folder syncs.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use daw_collab::bundle::{self, EXT};
use daw_collab::watch::{Change, FolderWatch};
use daw_project::{ProjectDoc, ProjectFolder};
use eframe::egui;

use crate::project::{After, find_projects, projects_dir};
use crate::{App, config_dir, user_dir};

/// How soon after an edit this computer's copy is written for others.
const SHARE_EVERY: Duration = Duration::from_secs(2);
/// How often the folder is looked at for other computers' copies.
const WATCH_EVERY: Duration = Duration::from_secs(2);

/// This computer's name in shared projects: made up once and kept in
/// the settings folder.
pub(crate) fn device_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        let path = config_dir().join("device-id.txt");
        if let Ok(s) = std::fs::read_to_string(&path) {
            let s = s.trim();
            if s.len() == 16 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
                return s.to_ascii_lowercase();
            }
        }
        let id = daw_project::new_id();
        let _ = std::fs::create_dir_all(config_dir());
        let _ = std::fs::write(&path, &id);
        id
    })
}

/// A bundle finished in the background.
pub(crate) enum Job {
    Exported(Result<PathBuf, String>),
    Imported(Result<Imported, String>),
}

pub(crate) struct Imported {
    name: String,
    dir: PathBuf,
    into: Dest,
    doc: Vec<u8>,
    /// Merged into the project on disk already.
    merged: bool,
}

/// Where a bundle goes.
#[derive(Clone, Copy, PartialEq)]
enum Dest {
    /// The open project is a copy of it: merged in place.
    Open,
    /// A copy of it among the user's projects: merged there, then opened.
    Saved,
    /// A project new to this computer: a new folder, then opened.
    New,
}

impl App {
    /// Writes this computer's copy of the document.
    pub(crate) fn save_device_copy(
        &mut self,
        folder: &ProjectFolder,
        bytes: &[u8],
    ) -> Result<(), String> {
        folder.save_device(device_id(), bytes)?;
        self.proj.device_version = self.proj.doc.version();
        self.proj.last_device_write = Instant::now();
        Ok(())
    }

    /// Starts watching the saved project's folder, if not already.
    pub(crate) fn watch_folder(&mut self) {
        let Some(folder) = &self.proj.folder else {
            return;
        };
        if self
            .proj
            .watch
            .as_ref()
            .is_some_and(|(d, _)| *d == folder.dir)
        {
            return;
        }
        let w = FolderWatch::start(folder.clone(), device_id(), WATCH_EVERY);
        self.proj.watch = Some((folder.dir.clone(), w));
    }

    /// Writes this computer's copy soon after an edit, so others see it
    /// before the next full save.
    pub(crate) fn share_soon(&mut self, ctx: &egui::Context) {
        let Some(folder) = self.proj.folder.clone() else {
            return;
        };
        if self.recording
            || self.proj.loading()
            || self.proj.last_device_write.elapsed() < SHARE_EVERY
            || ctx.input(|i| i.pointer.any_down())
        {
            return;
        }
        let version = self.proj.doc.version();
        if version == self.proj.device_version {
            return;
        }
        // Audio not in the folder yet would reach others as a path on
        // this computer: that waits for the save that brings it in.
        if (self.tracks.iter().flat_map(|t| &t.clips)).any(|c| !folder.has_asset(&c.asset)) {
            return;
        }
        let outside = |r: String| {
            !r.is_empty() && !r.starts_with(daw_effects::library::PREFIX) && !folder.has_asset(&r)
        };
        if (self.tracks.iter().flat_map(|t| &t.fx)).any(|s| outside(s.response_ref())) {
            return;
        }
        let bytes = self.proj.doc.to_bytes();
        if let Err(e) = self.save_device_copy(&folder, &bytes) {
            self.message = Some(e);
        }
    }

    /// Takes in other computers' changes and finished bundles.
    pub(crate) fn poll_sharing(&mut self, ctx: &egui::Context) {
        if let Some((_, w)) = &self.proj.watch {
            self.proj.incoming.extend(w.poll());
            ctx.request_repaint_after(WATCH_EVERY);
        }
        let busy = ctx.input(|i| i.pointer.any_down()) || ctx.egui_wants_keyboard_input();
        if !self.proj.incoming.is_empty() && !busy && !self.recording && !self.proj.loading() {
            self.merge_incoming(ctx);
        }

        let Some(rx) = &self.proj.bundle_job else {
            return;
        };
        ctx.request_repaint_after(Duration::from_millis(100));
        let job = match rx.try_recv() {
            Ok(j) => j,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Job::Exported(Err("it stopped".into())),
        };
        self.proj.bundle_job = None;
        match job {
            Job::Exported(Ok(path)) => {
                self.message = Some(format!("Bundle saved as {}", path.display()));
            }
            Job::Exported(Err(e)) => self.message = Some(format!("Couldn't export: {e}")),
            Job::Imported(Err(e)) => self.message = Some(format!("Couldn't import: {e}")),
            Job::Imported(Ok(i)) => self.finish_import(ctx, i),
        }
    }

    fn merge_incoming(&mut self, ctx: &egui::Context) {
        self.sync_doc(ctx, true);
        let before = self.proj.doc.version();
        let mut audio = false;
        let mut failed = Vec::new();
        for c in std::mem::take(&mut self.proj.incoming) {
            match c {
                Change::Doc { from, bytes } => {
                    if let Err(e) = self.merge_or_ask(&bytes) {
                        let name = from.file_name().unwrap_or_default().to_string_lossy();
                        failed.push(format!("{name}: {e}"));
                    }
                }
                Change::Audio => audio = true,
            }
        }
        // Audio another computer added may arrive after the clips that
        // use it: once it does, they are loaded again.
        let waiting = audio && !self.proj.missing_clips.is_empty();
        if self.proj.doc.version() != before || waiting {
            self.apply(self.proj.doc.get());
        }
        if !failed.is_empty() {
            self.message = Some(format!(
                "Couldn't merge another computer's changes ({})",
                failed.join("; ")
            ));
        }
    }

    /// Asks for a bundle and imports it.
    pub(crate) fn pick_bundle(&mut self) {
        let picked = rfd::FileDialog::new()
            .set_title("Import bundle")
            .set_directory(user_dir())
            .add_filter("Project bundle", &[EXT])
            .pick_file();
        if let Some(path) = picked {
            self.import_bundle(path);
        }
    }

    /// Saves the project, asks where, and writes its bundle in the
    /// background.
    pub(crate) fn export_bundle(&mut self, ctx: &egui::Context) {
        let Some(folder) = self.proj.folder.clone() else {
            self.message = Some("Save the project first, then export it as a bundle.".into());
            self.save_or_ask(ctx);
            return;
        };
        if let Err(e) = self.save(ctx) {
            self.message = Some(e);
            return;
        }
        let name = folder.name();
        let Some(dest) = rfd::FileDialog::new()
            .set_title("Export bundle")
            .set_directory(user_dir())
            .set_file_name(format!("{name}.{EXT}"))
            .add_filter("Project bundle", &[EXT])
            .save_file()
        else {
            return;
        };
        let dest = if bundle::is_bundle(&dest) {
            dest
        } else {
            let mut s = dest.into_os_string();
            s.push(format!(".{EXT}"));
            PathBuf::from(s)
        };
        // Frozen audio being made now belongs in the bundle: it is
        // written once that is done.
        if self.freezer.busy() {
            self.freezer.then_bundle = Some(dest);
            self.message = Some("Making frozen audio for the bundle first…".into());
            return;
        }
        self.write_bundle(dest);
    }

    /// Writes the saved project's bundle to `dest` in the background.
    pub(crate) fn write_bundle(&mut self, dest: PathBuf) {
        let Some(folder) = self.proj.folder.clone() else {
            return;
        };
        let name = folder.name();
        let doc = self.proj.doc.to_bytes();
        // Frozen audio, and audio that a snapshot would bring back, go too.
        let assets = self.proj.doc.assets_in_use();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let r = bundle::write(&dest, &name, &doc, &folder, &assets).map(|()| dest);
            let _ = tx.send(Job::Exported(r));
        });
        self.proj.bundle_job = Some(rx);
    }

    /// Imports a bundle: merged into this computer's copy of the project
    /// if there is one (open, or among the user's projects), otherwise
    /// made into a new project. The audio is unpacked in the background.
    pub(crate) fn import_bundle(&mut self, path: PathBuf) {
        if self.proj.bundle_job.is_some() {
            self.message = Some("A bundle is already being read or written.".into());
            return;
        }
        let b = match bundle::peek(&path) {
            Ok(b) => b,
            Err(e) => {
                self.message = Some(format!("Couldn't import: {e}"));
                return;
            }
        };
        let theirs = match ProjectDoc::from_bytes(&b.doc, daw_project::new_peer()) {
            Ok(d) => d,
            Err(e) => {
                self.message = Some(format!("Couldn't import {}: {e}", path.display()));
                return;
            }
        };
        let open = (self.proj.folder.as_ref())
            .filter(|_| daw_collab::related(&self.proj.doc, &theirs))
            .map(|f| f.dir.clone());
        let (into, dir) = if let Some(dir) = open {
            (Dest::Open, dir)
        } else if let Some(dir) = find_projects().into_iter().find(|d| {
            daw_collab::open_doc(d, daw_project::new_peer())
                .is_ok_and(|(_, doc, _)| daw_collab::related(&doc, &theirs))
        }) {
            (Dest::Saved, dir)
        } else {
            (Dest::New, new_folder_for(&b.name))
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(Job::Imported(unpack(&path, into, dir)));
        });
        self.proj.bundle_job = Some(rx);
    }

    fn finish_import(&mut self, ctx: &egui::Context, i: Imported) {
        match i.into {
            Dest::Open => {
                if self.proj.folder.as_ref().is_none_or(|f| f.dir != i.dir) {
                    self.message = Some(format!(
                        "The bundle's audio is in {}, but another project was opened, so its \
                         changes weren't merged. Import it again.",
                        i.dir.display()
                    ));
                    return;
                }
                self.sync_doc(ctx, true);
                match self.merge_or_ask(&i.doc) {
                    Ok(true) => {
                        self.message = Some(format!("Merged the changes in the {} bundle", i.name));
                    }
                    // Waiting for the owner to let its people in.
                    Ok(false) => {}
                    Err(e) => {
                        self.message = Some(format!("Couldn't import: {e}"));
                        return;
                    }
                }
                self.apply(self.proj.doc.get());
                self.proj.dirty = true;
            }
            Dest::Saved => {
                if !i.merged {
                    self.people.after_open = Some((i.dir.clone(), i.doc));
                }
                self.then(ctx, After::Open(i.dir));
                self.message = Some(format!(
                    "Merged the {} bundle into your copy of the project",
                    i.name
                ));
            }
            Dest::New => {
                self.then(ctx, After::Open(i.dir.clone()));
                self.message = Some(format!(
                    "Imported the {} bundle as a new project in {}",
                    i.name,
                    i.dir.display()
                ));
            }
        }
    }
}

/// Reads a bundle into `dir`, off the UI thread. A project that isn't
/// open has the bundle merged into it on disk here.
fn unpack(path: &std::path::Path, into: Dest, dir: PathBuf) -> Result<Imported, String> {
    let folder = match into {
        Dest::New => ProjectFolder::create(&dir)?,
        _ => ProjectFolder { dir: dir.clone() },
    };
    let b = bundle::unpack(path, &folder)?;
    let mut merged = true;
    match into {
        Dest::Open => merged = false,
        Dest::Saved => {
            let (folder, mut doc, _) = daw_collab::open_doc(&dir, daw_project::new_peer())?;
            // Changes from people not let in are merged once the project
            // is open, so its owner can be asked.
            if doc.check(&b.doc)?.ok() {
                doc.merge(&b.doc)?;
                let bytes = doc.to_bytes();
                if doc.take_dropped() {
                    folder.save_dropping_backup(&bytes)?;
                } else {
                    folder.save(&bytes)?;
                }
                folder.save_device(device_id(), &bytes)?;
            } else {
                merged = false;
            }
        }
        Dest::New => {
            // Checked when it was peeked at, but checked again: the file
            // may have changed since.
            ProjectDoc::from_bytes(&b.doc, daw_project::new_peer())?;
            folder.save(&b.doc)?;
        }
    }
    Ok(Imported {
        name: b.name,
        dir,
        into,
        doc: b.doc,
        merged,
    })
}

/// A new folder in the projects folder for a project called `name`
/// (as the bundle says, so made safe for a folder name).
fn new_folder_for(name: &str) -> PathBuf {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let clean = clean.trim().trim_matches('.').trim();
    let clean = if clean.is_empty() {
        "Imported project"
    } else {
        clean
    };
    let mut dir = projects_dir().join(clean);
    let mut n = 2;
    while dir.exists() {
        dir = projects_dir().join(format!("{clean} ({n})"));
        n += 1;
    }
    dir
}
