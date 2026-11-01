// SPDX-License-Identifier: GPL-3.0-or-later
//! A sample library read from an SFZ file ([`crate::sfz`]) with its
//! sounds. A library that fits in memory is read whole; a bigger one
//! keeps only the start of each sound in memory (enough to start a note
//! at once) and plays the rest from disk ([`crate::stream`]).
//!
//! Libraries are kept once per file while anything uses them, so undo,
//! a second track with the same kit, or rendering offline don't read
//! the sounds again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::SystemTime;

use daw_engine::import::Decoder;

use crate::sfz::{LoopMode, Sfz};

/// Frames of each sound kept in memory when the rest plays from disk:
/// about 0.19 s at 44.1 kHz, far longer than the disk takes to catch up.
pub const HEAD: usize = 8192;

/// Bytes of decoded sound a library may hold before it plays from disk.
pub const MEMORY: usize = 512 << 20;

/// Sound in memory: interleaved in one or two channels.
#[derive(Debug, Default)]
pub struct Pcm {
    pub data: Vec<f32>,
    pub channels: usize,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        self.data.len() / self.channels.max(1)
    }

    #[inline]
    pub fn frame(&self, i: usize) -> (f32, f32) {
        if self.channels == 2 {
            match self.data.get(2 * i..2 * i + 2) {
                Some(f) => (f[0], f[1]),
                None => (0.0, 0.0),
            }
        } else {
            let v = self.data.get(i).copied().unwrap_or(0.0);
            (v, v)
        }
    }

    pub fn peak(&self) -> f32 {
        self.data.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }
}

/// One sound file of a library.
#[derive(Debug)]
pub struct Source {
    pub path: PathBuf,
    /// Its rate, which needn't be a whole number for a sound made here.
    pub rate: f64,
    pub channels: usize,
    /// All of it.
    pub frames: u64,
    /// All of it, or its start when the rest plays from disk.
    pub pcm: Pcm,
    pub streamed: bool,
}

impl Source {
    /// A sound made here: a sine at middle C, or noise.
    fn made(name: &str) -> Option<Source> {
        let (data, rate) = match name {
            "*sine" => {
                // One cycle of 1024 frames, at the rate that makes it C4.
                let n = 1024;
                let data = (0..n)
                    .map(|i| (std::f32::consts::TAU * i as f32 / n as f32).sin())
                    .collect();
                (data, 1024.0 * 261.625_565)
            }
            "*noise" => {
                let mut x: u32 = 0x1234_5678;
                let data = (0..48_000)
                    .map(|_| {
                        x ^= x << 13;
                        x ^= x >> 17;
                        x ^= x << 5;
                        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
                    })
                    .collect();
                (data, 48_000.0)
            }
            _ => return None,
        };
        let pcm = Pcm { data, channels: 1 };
        Some(Source {
            path: PathBuf::from(name),
            rate,
            channels: 1,
            frames: pcm.frames() as u64,
            pcm,
            streamed: false,
        })
    }
}

/// An SFZ file and its sounds.
#[derive(Debug)]
pub struct Library {
    pub path: PathBuf,
    pub sfz: Sfz,
    pub sources: Vec<Source>,
    /// Each region's sound, by index in `sources`; `None` for one that
    /// couldn't be read (or `*silence`).
    pub region_source: Vec<Option<usize>>,
    /// The regions on each key, in the file's order.
    pub by_key: Vec<Vec<u32>>,
    /// Regions started by a controller.
    pub by_cc: Vec<u32>,
    /// Regions started by letting a key go.
    pub releases: Vec<Vec<u32>>,
    /// The keys that switch articulations.
    pub switches: Vec<bool>,
    /// The loudest sound's peak (of what is in memory).
    pub peak: f32,
    /// Bytes of sound in memory.
    pub bytes: usize,
    /// Whether some of it plays from disk.
    pub streamed: bool,
    /// Sounds that couldn't be read, and the file's own problems.
    pub problems: Vec<String>,
}

impl Library {
    /// A short name: the SFZ file's, without its folder or ending.
    pub fn name(&self) -> String {
        (self.path.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// The gain that brings the loudest sound's peak to −6 dBFS.
    pub fn normal_gain(&self) -> f32 {
        if self.peak > 1e-6 {
            0.5 / self.peak
        } else {
            1.0
        }
    }

    /// Reads `path` and its sounds, keeping what fits in `memory` bytes
    /// whole. Uses every core.
    pub fn read(path: &Path, memory: usize) -> Result<Library, String> {
        let sfz = crate::sfz::read(path)?;
        Ok(Library::of(path.to_path_buf(), sfz, memory))
    }

    /// The library for an SFZ already read.
    pub fn of(path: PathBuf, sfz: Sfz, memory: usize) -> Library {
        let mut problems = sfz.problems.clone();
        // The sounds, each once.
        let mut index: HashMap<&str, usize> = HashMap::new();
        let mut names: Vec<&str> = Vec::new();
        let mut needs: Vec<Need> = Vec::new();
        let mut region_name: Vec<Option<usize>> = Vec::with_capacity(sfz.regions.len());
        for r in &sfz.regions {
            if r.sample == "*silence" {
                region_name.push(None);
                continue;
            }
            let i = *index.entry(r.sample.as_str()).or_insert_with(|| {
                names.push(r.sample.as_str());
                needs.push(Need::default());
                names.len() - 1
            });
            let n = &mut needs[i];
            n.whole |= r.loop_mode == LoopMode::Continuous
                || r.loop_mode == LoopMode::Sustain
                || r.reverse
                || r.end.is_some();
            n.head = n.head.max((r.offset + r.offset_random) as usize + HEAD);
            region_name.push(Some(i));
        }
        // How big each is.
        let infos: Vec<Result<Info, String>> = parallel(&names, |name| {
            if name.starts_with('*') {
                return Ok((48_000.0, 1, Some(1)));
            }
            let d = Decoder::open(Path::new(name))?;
            Ok((d.rate as f64, d.channels, d.frames))
        });
        let total: u64 = (infos.iter())
            .map(|i| {
                i.as_ref()
                    .map_or(0, |(_, c, f)| f.unwrap_or(0) * *c as u64 * 4)
            })
            .sum();
        let stream = total as usize > memory;
        for (n, i) in needs.iter_mut().zip(&infos) {
            if let Ok((_, _, f)) = i {
                // A sound whose length isn't known, or that is short
                // anyway, is read whole.
                n.whole |= !stream || f.is_none_or(|f| f as usize <= n.head * 2);
            }
        }
        let read: Vec<Result<Source, String>> =
            parallel(&(0..names.len()).collect::<Vec<_>>(), |&i| {
                let name = names[i];
                if let Some(s) = Source::made(name) {
                    return Ok(s);
                }
                let (rate, channels, frames) = infos[i].clone()?;
                let mut d = Decoder::open(Path::new(name))?;
                let mut data = Vec::new();
                let whole = needs[i].whole;
                if whole {
                    while d.next(&mut data)? {}
                } else {
                    d.read_frames(needs[i].head, &mut data)?;
                    data.truncate(needs[i].head * channels);
                }
                let pcm = Pcm { data, channels };
                Ok(Source {
                    path: PathBuf::from(name),
                    rate,
                    channels,
                    frames: if whole {
                        pcm.frames() as u64
                    } else {
                        frames.unwrap_or(pcm.frames() as u64)
                    },
                    streamed: !whole,
                    pcm,
                })
            });
        let mut sources = Vec::with_capacity(read.len());
        let mut new_index: Vec<Option<usize>> = Vec::with_capacity(read.len());
        for (i, r) in read.into_iter().enumerate() {
            match r {
                Ok(s) => {
                    new_index.push(Some(sources.len()));
                    sources.push(s);
                }
                Err(e) => {
                    new_index.push(None);
                    if problems.len() < 50 {
                        problems.push(if e.contains(names[i]) {
                            e
                        } else {
                            format!("{}: {e}", names[i])
                        });
                    }
                }
            }
        }
        let region_source: Vec<Option<usize>> = (region_name.iter())
            .map(|n| n.and_then(|n| new_index[n]))
            .collect();
        let mut by_key = vec![Vec::new(); 128];
        let mut releases = vec![Vec::new(); 128];
        let mut by_cc = Vec::new();
        let mut switches = vec![false; 128];
        for (i, r) in sfz.regions.iter().enumerate() {
            if let Some((lo, hi)) = r.sw_range {
                for k in lo..=hi {
                    switches[k as usize] = true;
                }
            }
            if r.on_cc.is_some() {
                by_cc.push(i as u32);
            }
            if let Some((lo, hi)) = r.keys {
                for k in lo..=hi {
                    if r.trigger == crate::sfz::Trigger::Release {
                        releases[k as usize].push(i as u32);
                    } else {
                        by_key[k as usize].push(i as u32);
                    }
                }
            }
        }
        let peak = sources.iter().fold(0.0f32, |m, s| m.max(s.pcm.peak()));
        let bytes = sources.iter().map(|s| s.pcm.data.len() * 4).sum();
        Library {
            path,
            streamed: sources.iter().any(|s| s.streamed),
            sfz,
            sources,
            region_source,
            by_key,
            by_cc,
            releases,
            switches,
            peak,
            bytes,
            problems,
        }
    }
}

/// A sound file's rate, channels and length (if it says).
type Info = (f64, usize, Option<u64>);

#[derive(Default, Clone, Copy)]
struct Need {
    whole: bool,
    head: usize,
}

/// `f` of each item, spread over every core, in order.
fn parallel<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(16);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut out: Vec<(usize, R)> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(x) = items.get(i) else {
                            break;
                        };
                        mine.push((i, f(x)));
                    }
                    mine
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    });
    out.sort_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, r)| r).collect()
}

type Kept = Vec<(PathBuf, Option<SystemTime>, Weak<Library>)>;

static KEPT: Mutex<Kept> = Mutex::new(Vec::new());

/// The library at `path`: the one already in use if it hasn't changed
/// since, or read now (which can take a few seconds for a big one).
pub fn open(path: &Path) -> Result<Arc<Library>, String> {
    let changed = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    if let Ok(kept) = KEPT.lock()
        && let Some(l) = (kept.iter())
            .find(|(p, t, _)| p == path && *t == changed)
            .and_then(|(_, _, w)| w.upgrade())
    {
        return Ok(l);
    }
    let lib = Arc::new(Library::read(path, MEMORY)?);
    if let Ok(mut kept) = KEPT.lock() {
        kept.retain(|(p, _, w)| p != path && w.strong_count() > 0);
        kept.push((path.to_path_buf(), changed, Arc::downgrade(&lib)));
    }
    Ok(lib)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Writes a 16-bit WAV of `frames` frames of a rising ramp in
    /// `dir`, and returns its path.
    pub(crate) fn wav(dir: &Path, name: &str, frames: usize, level: f32) -> PathBuf {
        let p = dir.join(name);
        let mut w = hound::WavWriter::create(
            &p,
            hound::WavSpec {
                channels: 1,
                sample_rate: 48_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for i in 0..frames {
            let v = level * ((i % 1000) as f32 / 1000.0);
            w.write_sample((v * 32767.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        p
    }

    pub(crate) fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("daw-lib-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_library_reads_its_sounds_whole_or_just_their_starts() {
        let dir = scratch("read");
        wav(&dir, "a.wav", 100_000, 0.5);
        wav(&dir, "b.wav", 1000, 0.25);
        let text = "<region> sample=a.wav key=60\n<region> sample=b.wav key=61\n\
                    <region> sample=a.wav key=62 seq_length=2\n<region> sample=gone.wav key=63";
        std::fs::write(dir.join("kit.sfz"), text).unwrap();
        let lib = Library::read(&dir.join("kit.sfz"), MEMORY).unwrap();
        assert_eq!(lib.sources.len(), 2);
        assert_eq!(lib.region_source, [Some(0), Some(1), Some(0), None]);
        assert_eq!(lib.problems.len(), 1, "{:?}", lib.problems);
        assert!(!lib.streamed);
        assert_eq!(lib.sources[0].pcm.frames(), 100_000);
        assert!((lib.peak - 0.5).abs() < 0.01);
        assert_eq!(lib.by_key[60], [0]);
        // With almost no memory, the long one plays from disk.
        let lib = Library::read(&dir.join("kit.sfz"), 1000).unwrap();
        assert!(lib.streamed && lib.sources[0].streamed && !lib.sources[1].streamed);
        assert_eq!(lib.sources[0].pcm.frames(), HEAD);
        assert_eq!(lib.sources[0].frames, 100_000);
        // Kept once while in use.
        let a = open(&dir.join("kit.sfz")).unwrap();
        let b = open(&dir.join("kit.sfz")).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
    }
}
