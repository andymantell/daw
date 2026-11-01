// SPDX-License-Identifier: GPL-3.0-or-later
//! Playing a big library's sounds from disk. Each voice that plays a
//! sound kept only in part ([`crate::library::HEAD`] frames) has a ring
//! of frames that a loader thread fills from the file, from where the
//! part in memory ends. The voice plays the start from memory, then the
//! ring. Nothing on the audio thread reads a file, allocates or waits:
//! if the disk falls behind, the voice holds still for that sample
//! (counted in [`Shared::underruns`]) rather than play a gap.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use daw_engine::import::Decoder;
use rtrb::{Consumer, Producer, RingBuffer};

use crate::library::{Library, Source};

/// Frames each voice's ring holds: about a third of a second at
/// 44.1 kHz.
pub const RING: usize = 16_384;

#[derive(Clone, Copy, Default)]
pub struct Frame {
    /// Which start of the voice it belongs to: frames left from the
    /// sound a voice played before are passed over.
    generation: u32,
    l: f32,
    r: f32,
}

enum Msg {
    Start {
        slot: usize,
        generation: u32,
        source: usize,
        from: u64,
    },
    Stop {
        slot: usize,
    },
}

/// What the loader and the instrument share.
#[derive(Default)]
pub struct Shared {
    alive: AtomicBool,
    /// Samples a voice waited for the disk.
    pub underruns: AtomicU64,
}

/// The audio thread's side: a ring for each voice, and a queue to the
/// loader.
pub struct Streams {
    rings: Vec<Consumer<Frame>>,
    send: Producer<Msg>,
    generations: Vec<u32>,
    pub shared: Arc<Shared>,
}

impl Streams {
    /// Rings for `voices` voices of `library`, and its loader thread.
    /// Allocates and starts a thread; not for the audio thread.
    pub fn new(library: Arc<Library>, voices: usize) -> Streams {
        let (mut rings, mut fills) = (Vec::new(), Vec::new());
        for _ in 0..voices {
            let (p, c) = RingBuffer::new(RING);
            rings.push(c);
            fills.push(p);
        }
        let (send, recv) = RingBuffer::new(voices * 4);
        let shared = Arc::new(Shared {
            alive: AtomicBool::new(true),
            ..Shared::default()
        });
        let s = shared.clone();
        let _ = std::thread::Builder::new()
            .name("sampler-disk".into())
            .spawn(move || load(library, fills, recv, s));
        Streams {
            rings,
            send,
            generations: vec![0; voices],
            shared,
        }
    }

    /// Asks for voice `slot` to play `source` from frame `from` on.
    /// The cursor that reads it.
    pub fn start(&mut self, slot: usize, source: usize, from: u64, head: u64) -> Cursor {
        let g = self.generations[slot].wrapping_add(1);
        self.generations[slot] = g;
        let from = from.max(head);
        let _ = self.send.push(Msg::Start {
            slot,
            generation: g,
            source,
            from,
        });
        Cursor::new(g, from)
    }

    /// The voice in `slot` has stopped.
    pub fn stop(&mut self, slot: usize) {
        let _ = self.send.push(Msg::Stop { slot });
    }

    pub fn ring(&mut self, slot: usize) -> &mut Consumer<Frame> {
        &mut self.rings[slot]
    }
}

impl Drop for Streams {
    fn drop(&mut self) {
        self.shared.alive.store(false, Ordering::Relaxed);
    }
}

/// Where a voice is in a sound that plays from disk, with the last few
/// frames read, for reading between them.
#[derive(Clone, Copy)]
pub struct Cursor {
    generation: u32,
    /// The ring's first frame: frames before it (and after the part in
    /// memory) are silent.
    from: u64,
    /// The next frame the ring gives.
    next: u64,
    recent: [(f32, f32); 8],
}

impl Cursor {
    fn new(generation: u32, from: u64) -> Cursor {
        Cursor {
            generation,
            from,
            next: from,
            recent: [(0.0, 0.0); 8],
        }
    }

    /// Frame `k` of `s`; `None` when the disk hasn't caught up.
    #[inline]
    pub fn frame(&mut self, k: u64, s: &Source, ring: &mut Consumer<Frame>) -> Option<(f32, f32)> {
        let head = s.pcm.frames() as u64;
        if k < head {
            return Some(s.pcm.frame(k as usize));
        }
        if k >= s.frames || k < self.from {
            return Some((0.0, 0.0));
        }
        if k < self.next {
            return Some(if self.next - k <= 8 {
                self.recent[(k % 8) as usize]
            } else {
                (0.0, 0.0)
            });
        }
        while self.next <= k {
            match ring.pop() {
                Ok(f) if f.generation == self.generation => {
                    self.recent[(self.next % 8) as usize] = (f.l, f.r);
                    self.next += 1;
                }
                Ok(_) => {}
                Err(_) => return None,
            }
        }
        Some(self.recent[(k % 8) as usize])
    }
}

struct Job {
    generation: u32,
    decoder: Decoder,
    channels: usize,
    /// Decoded and not yet in the ring.
    buf: Vec<f32>,
    /// The frame `buf` starts at, and how many of it are in the ring.
    pos: u64,
    at: usize,
    from: u64,
}

impl Job {
    fn open(s: &Source, generation: u32, from: u64) -> Option<Job> {
        let decoder = Decoder::open(&s.path).ok()?;
        Some(Job {
            generation,
            channels: decoder.channels.max(1),
            decoder,
            buf: Vec::new(),
            pos: 0,
            at: 0,
            from,
        })
    }

    /// Moves frames into `ring` while it has room. Whether it moved
    /// any, and whether there is more to come.
    fn fill(&mut self, ring: &mut Producer<Frame>) -> (bool, bool) {
        let mut moved = false;
        loop {
            let frames = self.buf.len() / self.channels;
            while self.at < frames && !ring.is_full() {
                let i = self.at * self.channels;
                let (l, r) = if self.channels == 2 {
                    (self.buf[i], self.buf[i + 1])
                } else {
                    (self.buf[i], self.buf[i])
                };
                let _ = ring.push(Frame {
                    generation: self.generation,
                    l,
                    r,
                });
                self.at += 1;
                moved = true;
            }
            if self.at < frames {
                return (moved, true);
            }
            self.pos += frames as u64;
            self.buf.clear();
            self.at = 0;
            match self.decoder.next(&mut self.buf) {
                Ok(true) => {
                    let n = (self.buf.len() / self.channels) as u64;
                    if self.pos + n <= self.from {
                        // Before where the voice wants it.
                        self.at = n as usize;
                    } else if self.pos < self.from {
                        self.at = (self.from - self.pos) as usize;
                    }
                }
                _ => return (moved, false),
            }
        }
    }
}

fn load(
    library: Arc<Library>,
    mut fills: Vec<Producer<Frame>>,
    mut recv: Consumer<Msg>,
    shared: Arc<Shared>,
) {
    let mut jobs: Vec<Option<Job>> = (0..fills.len()).map(|_| None).collect();
    while shared.alive.load(Ordering::Relaxed) {
        let mut busy = false;
        while let Ok(m) = recv.pop() {
            busy = true;
            match m {
                Msg::Start {
                    slot,
                    generation,
                    source,
                    from,
                } => {
                    jobs[slot] =
                        (library.sources.get(source)).and_then(|s| Job::open(s, generation, from));
                }
                Msg::Stop { slot } => jobs[slot] = None,
            }
        }
        for (slot, job) in jobs.iter_mut().enumerate() {
            let Some(j) = job else {
                continue;
            };
            let (moved, more) = j.fill(&mut fills[slot]);
            busy |= moved;
            if !more {
                *job = None;
            }
        }
        if !busy {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::tests::{scratch, wav};

    #[test]
    fn a_sound_played_from_disk_matches_the_same_sound_in_memory() {
        let dir = scratch("stream");
        wav(&dir, "long.wav", 60_000, 0.8);
        std::fs::write(dir.join("k.sfz"), "<region> sample=long.wav").unwrap();
        let whole = Library::read(&dir.join("k.sfz"), usize::MAX).unwrap();
        let lib = Arc::new(Library::read(&dir.join("k.sfz"), 0).unwrap());
        assert!(lib.sources[0].streamed);
        let mut st = Streams::new(lib.clone(), 2);
        let s = &lib.sources[0];
        // A voice before plays something else, which must not show.
        let _ = st.start(0, 0, 30_000, s.pcm.frames() as u64);
        let mut c = st.start(0, 0, 0, s.pcm.frames() as u64);
        let mut k = 0u64;
        let mut waited = 0;
        while k < 60_000 {
            match c.frame(k, s, st.ring(0)) {
                Some(v) => {
                    assert_eq!(v, whole.sources[0].pcm.frame(k as usize), "frame {k}");
                    k += 1;
                }
                None => {
                    waited += 1;
                    assert!(waited < 10_000, "the disk never caught up");
                    std::thread::sleep(Duration::from_micros(200));
                }
            }
        }
        assert_eq!(c.frame(60_000, s, st.ring(0)), Some((0.0, 0.0)));
    }
}
