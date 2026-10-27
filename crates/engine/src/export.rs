// SPDX-License-Identifier: GPL-3.0-or-later
//! Writing a mix to a WAV file (roadmap 1.8): the whole graph for a
//! mixdown, or a graph of one track for a stem.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::engine::render_with;
use crate::graph::Graph;
use crate::time::Frames;

/// Renders `frames` frames of `graph` from `from` into a stereo 24-bit
/// WAV file at `path`. `done` counts frames written so far; setting
/// `cancel` stops early and removes the file. The file appears only when
/// complete: it is written beside `path` first and renamed.
pub fn write_wav(
    graph: Graph,
    from: Frames,
    frames: usize,
    path: &Path,
    done: &AtomicU64,
    cancel: &AtomicBool,
) -> Result<Written, String> {
    let rate = graph.tempo().sample_rate();
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: 24,
        sample_format: hound::SampleFormat::Int,
    };
    let tmp = path.with_extension("wav.part");
    let mut w = hound::WavWriter::create(&tmp, spec)
        .map_err(|e| format!("could not create {}: {e}", tmp.display()))?;
    let mut error = None;
    let mut peak = 0.0f32;
    const FULL: f32 = 8_388_607.0;
    let block = graph.max_block();
    render_with(graph, from, frames, block, |l, r| {
        for (a, b) in l.iter().zip(r) {
            for x in [a, b] {
                peak = peak.max(x.abs());
                let v = (x.clamp(-1.0, 1.0) * FULL).round() as i32;
                if let Err(e) = w.write_sample(v) {
                    error = Some(e.to_string());
                    return false;
                }
            }
        }
        done.fetch_add(l.len() as u64, Ordering::Relaxed);
        !cancel.load(Ordering::Relaxed)
    });
    let finished = w.finalize().map_err(|e| e.to_string());
    if let Some(e) = error.or(finished.err()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("could not write {}: {e}", path.display()));
    }
    if cancel.load(Ordering::Relaxed) {
        let _ = std::fs::remove_file(&tmp);
        return Err("cancelled".into());
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("could not save {}: {e}", path.display()))?;
    Ok(Written { peak })
}

/// What [`write_wav`] wrote.
#[derive(Clone, Copy, Debug)]
pub struct Written {
    /// Highest sample level before clipping to full scale.
    pub peak: f32,
}

impl Written {
    pub fn clipped(&self) -> bool {
        self.peak > 1.0
    }
}
