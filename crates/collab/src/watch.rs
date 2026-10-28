// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared-folder sync: watching a project folder for what other
//! computers write into it.
//!
//! The folder may sit in any synced folder, so nothing about the
//! provider is assumed. Files are looked at every so often rather than
//! through the system's change notices, which synced folders don't
//! always send. A file is passed on once it has stopped changing (same
//! size and time on two looks), so one still arriving isn't read half
//! written. Only other computers' copies in `Devices` are read: ours is
//! skipped, and `project.daw` may be anyone's last save, which their
//! own copy holds as well.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

use daw_project::{ASSETS, ProjectFolder};

/// Something that arrived in the folder.
pub enum Change {
    /// Another computer's copy of the document, to merge.
    Doc { from: PathBuf, bytes: Vec<u8> },
    /// Audio files came or went.
    Audio,
}

/// Watches a folder until dropped.
pub struct FolderWatch {
    rx: mpsc::Receiver<Change>,
    stop: Arc<AtomicBool>,
}

impl FolderWatch {
    /// Starts watching `folder` every `every`, skipping the copy that
    /// `device` (this computer) writes. Copies already there are passed
    /// on too, as they may have changed since the project was opened.
    pub fn start(folder: ProjectFolder, device: &str, every: Duration) -> FolderWatch {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let mine = folder.device_file(device);
        let stopped = stop.clone();
        std::thread::spawn(move || {
            let mut w = Watcher {
                folder,
                mine,
                seen: HashMap::new(),
                sent: HashMap::new(),
                audio: None,
            };
            while !stopped.load(Ordering::Relaxed) {
                for c in w.look() {
                    if tx.send(c).is_err() {
                        return;
                    }
                }
                std::thread::sleep(every);
            }
        });
        FolderWatch { rx, stop }
    }

    /// What has arrived since last asked.
    pub fn poll(&self) -> Vec<Change> {
        self.rx.try_iter().collect()
    }
}

impl Drop for FolderWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

type Stamp = (Option<SystemTime>, u64);

struct Watcher {
    folder: ProjectFolder,
    mine: PathBuf,
    /// How each file looked last time.
    seen: HashMap<PathBuf, Stamp>,
    /// How each file looked when it was last passed on.
    sent: HashMap<PathBuf, Stamp>,
    audio: Option<BTreeSet<String>>,
}

impl Watcher {
    fn look(&mut self) -> Vec<Change> {
        let mut out = Vec::new();
        let mut now = HashMap::new();
        for f in self.folder.device_files() {
            if f == self.mine {
                continue;
            }
            let Ok(m) = std::fs::metadata(&f) else {
                continue;
            };
            let stamp = (m.modified().ok(), m.len());
            let settled = self.seen.get(&f) == Some(&stamp);
            if settled
                && self.sent.get(&f) != Some(&stamp)
                && let Ok(bytes) = std::fs::read(&f)
            {
                self.sent.insert(f.clone(), stamp);
                out.push(Change::Doc {
                    from: f.clone(),
                    bytes,
                });
            }
            now.insert(f, stamp);
        }
        self.seen = now;

        let audio: BTreeSet<String> = std::fs::read_dir(self.folder.dir.join(ASSETS))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| daw_project::is_asset_name(n))
            .collect();
        if self.audio.as_ref().is_some_and(|a| *a != audio) {
            out.push(Change::Audio);
        }
        self.audio = Some(audio);
        out
    }
}
