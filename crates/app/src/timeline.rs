// SPDX-License-Identifier: GPL-3.0-or-later
//! The timeline (roadmap 1.5, 2.1): each track's clips on a lane, with
//! their waveforms. Select, move, trim, slip, split and delete clips,
//! snapped to a grid that follows the tempo map, or to other clips and
//! markers; fade their ends, crossfade where they meet, and set their
//! volume with a line of points. Ripple editing closes the gaps that
//! deleting and trimming leave. Markers on bar lines name the song's
//! sections, and a section can be looped. Clips can be normalised,
//! reversed or have their gain changed. Clips on grouped tracks are
//! edited together (see `groups.rs`). Comments show as pins. Undo
//! belongs to the project document; the timeline only asks for it.

use crate::icons::{Icon, IconButton};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;

use daw_engine::fade::{Curve, Fade, GainPoint, Shape, envelope_db};
use daw_engine::graph::db_to_gain;
use daw_engine::process::Process;
use daw_engine::stream::StreamSource;
use daw_engine::time::{Frames, PPQ as ENGINE_PPQ, TempoMap, Ticks};
use daw_engine::warp::{Warp, WarpPoint};
use eframe::egui::{
    self, Align2, Color32, CursorIcon, FontId, Key, Modifiers, Pos2, Rect, RichText, Sense, Stroke,
    StrokeKind, Vec2, pos2, vec2,
};

use crate::commands::Cmd;
use crate::groups::{self, LaneGroup};
use crate::theme::{Look, LoopFill, Texture, WaveStyle};
use crate::versions::VersionAsk;
use daw_project::{NoteClip, PPQ, StretchMarker};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Part of a file placed on a track.
#[derive(Clone, Debug)]
pub struct Clip {
    /// Unique for the life of the process; selection follows it.
    pub id: u64,
    /// The clip's id in the project document.
    pub key: String,
    /// Its audio in the project document: an asset's name once saved,
    /// the file's full path before.
    pub asset: String,
    pub source: Arc<StreamSource>,
    /// The file as given: the original of a converted copy, or the take.
    /// Opened again if the device changes rate.
    pub path: PathBuf,
    /// What the file was called when it came in; the saved copy is named
    /// by its content.
    pub label: String,
    /// Where it plays on the timeline, in frames.
    pub start: i64,
    /// The first frame of the source that plays.
    pub offset: usize,
    /// How many frames play.
    pub len: usize,
    /// Which recorded take this is part of (from 0); `None` for a file.
    pub take: Option<usize>,
    /// Out of the mix: an older take under a newer one.
    pub muted: bool,
    /// A piece of the track's comp, cut from take `take` (see
    /// [`crate::comping`]).
    pub comp: bool,
    /// The clip's own volume, in dB.
    pub gain_db: f32,
    /// Fades at the start and end, in frames; together never longer
    /// than the clip.
    pub fade_in: Fade,
    pub fade_out: Fade,
    /// Volume points, in source frames, in order: the line stays with
    /// the audio when the clip is trimmed, split or slipped.
    pub envelope: Vec<GainPoint>,
    /// The clip's own tempo in beats per minute once it follows the
    /// song's tempo: it is stretched from this tempo to the song's, and
    /// its start and markers are in ticks. `None` keeps it in frames.
    pub tempo: Option<f64>,
    /// Where it starts in ticks when it follows tempo, kept in step
    /// with `start` by the app (`start` follows `beat` when the tempo
    /// changes, `beat` follows `start` when the clip is moved).
    pub beat: i64,
    /// Stretch markers: source frames tied to points from the clip's
    /// start, in ticks when it follows tempo, else in timeline frames.
    /// In order of frame, all strictly inside the clip's audio. The
    /// audio between two is stretched evenly; past the last it plays at
    /// the clip's own tempo.
    pub markers: Vec<StretchMarker>,
    /// The clip's own transpose in cents (100 to a semitone), within an
    /// octave either way; see [`crate::transpose`].
    pub cents: i32,
    /// A transpose moves the formants with the pitch instead of keeping
    /// them where they were.
    pub move_formants: bool,
    /// Pitch editing: the notes found in the clip's audio, with their
    /// edits, in source frames; see [`crate::pitch_edit`].
    pub pitch_notes: Vec<daw_project::PitchNote>,
    /// Linked copies share a link: see `daw_project::sync_links`.
    pub link: Option<String>,
    /// The pitch line of its audio, once worked out: what its notes are
    /// drawn over and their drift and vibrato edits work from.
    pub pitch_line: Option<Arc<daw_engine::bend::PitchLine>>,
    /// How many timeline frames the clip takes once stretched; `len`
    /// for a clip that isn't. Kept up to date by [`Clip::retime`].
    pub span: usize,
    /// Clip time units per timeline frame where it starts, under the
    /// song's tempo map; kept by [`Clip::retime`]. 0 until then.
    pub out_unit: f64,
}

/// The shortest clip a trim or split leaves, in frames.
pub const MIN_LEN: usize = 64;

/// The steepest stretch a marker may ask for against the clip's own
/// tempo, either way: within what the stretcher takes, with room for
/// the song's tempo to differ from the clip's.
pub const MAX_STRETCH: f64 = 3.0;

/// The range of a clip's volume and its points, in dB.
pub const MIN_DB: f32 = -48.0;
pub const MAX_DB: f32 = 12.0;

/// How long a crossfade made where two clips just touch lasts.
pub const CROSSFADE_SECONDS: f64 = 0.02;

/// Where two clips on a track meet: fades the earlier out and the later
/// in across their overlap, with steady power. Clips that touch (or
/// nearly) are first stretched into each other by half of `len` each,
/// as far as their audio goes. False if they don't meet.
pub fn crossfade(a: &mut Clip, b: &mut Clip, len: usize) -> bool {
    if b.start < a.start {
        return crossfade(b, a, len);
    }
    let gap = b.start - a.end();
    if gap > (len / 2) as i64 {
        return false;
    }
    if gap >= 0 {
        let half = (len / 2) as i64;
        let meet = a.end() + gap / 2;
        a.trim_end(meet + half);
        b.trim_start(meet - half);
    }
    // One inside the other has no join to fade across.
    if b.end() <= a.end() {
        return false;
    }
    let overlap = (a.end() - b.start).max(0) as usize;
    if overlap == 0 {
        return false;
    }
    a.fade_out = Fade::new(overlap, Curve::EqualPower);
    b.fade_in = Fade::new(overlap, Curve::EqualPower);
    a.fit_fades();
    b.fit_fades();
    true
}

impl Clip {
    /// All of `source`, starting at `start`. Anything before the start of
    /// the timeline is trimmed off.
    pub fn new(source: Arc<StreamSource>, path: PathBuf, start: i64, take: Option<usize>) -> Clip {
        let mut c = Clip {
            id: next_id(),
            key: daw_project::new_id(),
            asset: path.to_string_lossy().into_owned(),
            label: path
                .file_stem()
                .map_or(String::new(), |s| s.to_string_lossy().into_owned()),
            len: source.frames,
            source,
            path,
            start,
            offset: 0,
            take,
            muted: false,
            comp: false,
            gain_db: 0.0,
            fade_in: Fade::default(),
            fade_out: Fade::default(),
            envelope: Vec::new(),
            tempo: None,
            beat: 0,
            markers: Vec::new(),
            cents: 0,
            move_formants: false,
            pitch_notes: Vec::new(),
            link: None,
            pitch_line: None,
            span: 0,
            out_unit: 0.0,
        };
        c.span = c.len;
        if c.start < 0 {
            let cut = ((-c.start) as usize).min(c.len);
            c.offset = cut;
            c.len -= cut;
            c.start = 0;
        }
        c
    }

    /// Where the clip ends on the timeline.
    pub fn end(&self) -> i64 {
        if self.is_stretched() {
            self.start + self.span as i64
        } else {
            self.start + self.len as i64
        }
    }

    /// Whether the clip is played through the stretcher: it follows
    /// tempo, or has stretch markers.
    pub fn is_stretched(&self) -> bool {
        self.tempo.is_some() || !self.markers.is_empty()
    }

    /// The clip's time units per source frame at its own tempo: ticks
    /// when it follows tempo, else frames (1).
    fn unit(&self) -> f64 {
        match self.tempo {
            Some(bpm) => bpm * PPQ as f64 / 60.0 / self.source.rate.max(1) as f64,
            None => 1.0,
        }
    }

    /// The points tying source frames to the clip's time, from its
    /// first frame (at 0) through its markers.
    fn ties(&self) -> impl Iterator<Item = (f64, f64)> + '_ {
        std::iter::once((self.offset as f64, 0.0)).chain(
            (self.markers.iter())
                .filter(|m| m.frame > self.offset as i64)
                .map(|m| (m.frame as f64, m.at as f64)),
        )
    }

    /// Where source frame `frame` falls in the clip's own time, counted
    /// from its start.
    pub fn at_of_frame(&self, frame: f64) -> f64 {
        let mut last = (self.offset as f64, 0.0);
        for (f, at) in self.ties().skip(1) {
            if frame <= f {
                let k = (frame - last.0) / (f - last.0).max(1.0);
                return last.1 + k * (at - last.1);
            }
            last = (f, at);
        }
        last.1 + (frame - last.0) * self.unit()
    }

    /// The source frame at `at` in the clip's own time.
    pub fn frame_of_at(&self, at: f64) -> f64 {
        let mut last = (self.offset as f64, 0.0);
        for (f, a) in self.ties().skip(1) {
            if at <= a {
                let k = (at - last.1) / (a - last.1).max(1e-9);
                return last.0 + k * (f - last.0);
            }
            last = (f, a);
        }
        last.0 + (at - last.1) / self.unit()
    }

    /// The timeline frame source frame `frame` plays on.
    pub fn out_of_frame(&self, map: &TempoMap, frame: f64) -> i64 {
        let at = self.at_of_frame(frame);
        match self.tempo {
            Some(_) => map.frames_at(Ticks(self.beat + at.round() as i64)).0,
            None => self.start + at.round() as i64,
        }
    }

    /// The source frame playing on timeline frame `out`.
    pub fn frame_of_out(&self, map: &TempoMap, out: i64) -> f64 {
        self.frame_of_at(self.at_of_out(map, out))
    }

    /// Timeline frame `out` in the clip's own time.
    pub fn at_of_out(&self, map: &TempoMap, out: i64) -> f64 {
        match self.tempo {
            Some(_) => (map.ticks_at(Frames(out)).0 - self.beat) as f64,
            None => (out - self.start) as f64,
        }
    }

    /// Works out how long the clip is on the timeline under `map`.
    pub fn retime(&mut self, map: &TempoMap) {
        self.out_unit = match self.tempo {
            Some(_) => {
                map.tempo_at(Ticks(self.beat)).as_f64() * PPQ as f64
                    / 60.0
                    / map.sample_rate().max(1) as f64
            }
            None => 1.0,
        };
        self.span = if self.is_stretched() {
            let end = self.out_of_frame(map, (self.offset + self.len) as f64);
            (end - self.start).max(MIN_LEN as i64) as usize
        } else {
            self.len
        };
    }

    /// How the engine plays the clip under `map`: its ties as a warp
    /// from source frames to timeline frames, with a point on every
    /// beat when the song's tempo changes, so the audio follows the
    /// map between markers too. `None` for a clip played plainly, or
    /// whose markers ask for more than the stretcher can do.
    pub fn warp(&self, map: &TempoMap) -> Option<Warp> {
        if !self.is_stretched() {
            return None;
        }
        let changes = self.tempo.is_some() && map.tempo_changes().next().is_some();
        let mut points: Vec<WarpPoint> = Vec::new();
        let mut push = |src: i64, out: i64| {
            if points.last().is_none_or(|p| src > p.src && out > p.out) {
                points.push(WarpPoint { src, out });
            }
        };
        let end = (self.offset + self.len) as f64;
        let ties: Vec<(f64, f64)> = self
            .ties()
            .chain(std::iter::once((end, self.at_of_frame(end))))
            .collect();
        for w in ties.windows(2) {
            let (f0, a0) = w[0];
            let (f1, a1) = w[1];
            push(f0.round() as i64, self.out_of_frame(map, f0));
            if changes {
                // Every beat line strictly between the two ties.
                let ppq = ENGINE_PPQ as f64;
                let mut beat = ((self.beat as f64 + a0) / ppq).floor() + 1.0;
                while beat * ppq < self.beat as f64 + a1 {
                    let at = beat * ppq - self.beat as f64;
                    let src = self.frame_of_at(at).round() as i64;
                    let out = map.frames_at(Ticks((beat * ppq) as i64)).0;
                    push(src, out);
                    beat += 1.0;
                }
            }
            push(f1.round() as i64, self.out_of_frame(map, f1));
        }
        Warp::new(points).ok()
    }

    /// Adds a stretch marker at source frame `frame`, keeping the
    /// timing there as it is, or finds the one already there. `None`
    /// outside the clip's audio.
    pub fn add_marker(&mut self, frame: i64) -> Option<usize> {
        if frame <= self.offset as i64 || frame >= (self.offset + self.len) as i64 {
            return None;
        }
        match self.markers.binary_search_by_key(&frame, |m| m.frame) {
            Ok(i) => Some(i),
            Err(i) => {
                let at = self.at_of_frame(frame as f64).round() as i64;
                self.markers.insert(i, StretchMarker { frame, at });
                Some(i)
            }
        }
    }

    /// Moves marker `i` so its frame plays at `at` in the clip's own
    /// time, kept between its neighbours and within [`MAX_STRETCH`].
    /// True if it moved.
    pub fn move_marker(&mut self, i: usize, at: i64) -> bool {
        let Some(m) = self.markers.get(i).copied() else {
            return false;
        };
        let u = self.unit();
        let (f0, a0) = match i.checked_sub(1).and_then(|j| self.markers.get(j)) {
            Some(p) => (p.frame as f64, p.at as f64),
            None => (self.offset as f64, 0.0),
        };
        let gap = m.frame as f64 - f0;
        let mut lo = a0 + gap * u / MAX_STRETCH;
        let mut hi = a0 + gap * u * MAX_STRETCH;
        if let Some(n) = self.markers.get(i + 1) {
            let gap = (n.frame - m.frame) as f64;
            lo = lo.max(n.at as f64 - gap * u * MAX_STRETCH);
            hi = hi.min(n.at as f64 - gap * u / MAX_STRETCH);
        }
        let at = (at as f64)
            .clamp(lo.ceil(), hi.floor().max(lo.ceil()))
            .round() as i64;
        let moved = at != m.at;
        self.markers[i].at = at;
        moved
    }

    /// Drops the markers outside the clip's audio, and any that would
    /// run backwards after an edit.
    fn prune_markers(&mut self) {
        let (lo, hi) = (self.offset as i64, (self.offset + self.len) as i64);
        self.markers.retain(|m| m.frame > lo && m.frame < hi);
        let mut last = 0;
        self.markers.retain(|m| {
            let keep = m.at > last;
            if keep {
                last = m.at;
            }
            keep
        });
    }

    /// Makes the clip follow the song's tempo at the tempo in force
    /// where it starts, or stop following it, keeping its timing under
    /// `map` as it is either way.
    pub fn follow_tempo(&mut self, map: &TempoMap, follow: bool) {
        if follow == self.tempo.is_some() {
            return;
        }
        if follow {
            let beat = map.ticks_at(Frames(self.start));
            let bpm = map.tempo_at(beat).as_f64();
            for m in &mut self.markers {
                // Where it plays now, as ticks from the start.
                let out = self.start + m.at;
                m.at = map.ticks_at(Frames(out)).0 - beat.0;
            }
            self.beat = beat.0;
            self.tempo = Some(bpm);
        } else {
            let outs: Vec<i64> = (self.markers.iter())
                .map(|m| self.out_of_frame(map, m.frame as f64) - self.start)
                .collect();
            for (m, out) in self.markers.iter_mut().zip(outs) {
                m.at = out;
            }
            self.tempo = None;
        }
        self.prune_markers();
        self.retime(map);
    }

    /// Changes the clip's own tempo: the markers keep their places in
    /// the audio, and the clip is stretched by the new ratio.
    pub fn set_tempo(&mut self, map: &TempoMap, bpm: f64) {
        let Some(old) = self.tempo else {
            return;
        };
        let bpm = bpm.clamp(20.0, 400.0);
        let k = bpm / old;
        for m in &mut self.markers {
            m.at = (m.at as f64 * k).round() as i64;
        }
        self.tempo = Some(bpm);
        self.prune_markers();
        self.retime(map);
    }

    fn overlaps(&self, other: &Clip) -> bool {
        self.start < other.end() && other.start < self.end()
    }

    /// Moves the start edge to `to`, keeping the audio where it is: as
    /// far back as the source goes (and no earlier than the timeline's
    /// start), and up to [`MIN_LEN`] short of the end.
    pub fn trim_start(&mut self, to: i64) {
        let lo = (self.start - self.offset as i64).max(0);
        let hi = self.end() - MIN_LEN as i64;
        if hi < lo {
            return;
        }
        let d = to.clamp(lo, hi) - self.start;
        if self.is_stretched() {
            // The edge moves in timeline frames; the audio under it by
            // the stretch there.
            let frame = self
                .frame_of_at(self.at_of_frame(self.offset as f64) + d as f64 * self.unit_per_out());
            let new_offset =
                (frame.round() as i64).clamp(0, (self.offset + self.len - MIN_LEN) as i64);
            let shift = self.at_of_frame(new_offset as f64).round() as i64;
            for m in &mut self.markers {
                m.at -= shift;
            }
            if self.tempo.is_some() {
                self.beat += shift;
            }
            let d_src = new_offset - self.offset as i64;
            self.offset = new_offset as usize;
            self.len = (self.len as i64 - d_src) as usize;
            self.span = (self.span as i64 - d).max(MIN_LEN as i64) as usize;
            self.start += d;
            self.prune_markers();
        } else {
            self.offset = (self.offset as i64 + d) as usize;
            self.len = (self.len as i64 - d) as usize;
            self.start += d;
        }
        self.fit_fades();
    }

    /// Clip time units per timeline frame near the clip's start under
    /// the song's tempo, as last retimed; the clip's own tempo until
    /// then.
    fn unit_per_out(&self) -> f64 {
        if self.out_unit > 0.0 {
            self.out_unit
        } else {
            self.unit()
        }
    }

    /// Moves the end edge to `to`, as far as the source goes.
    pub fn trim_end(&mut self, to: i64) {
        let most = self.source.frames - self.offset;
        if self.is_stretched() {
            let span = (to - self.start).max(MIN_LEN as i64) as usize;
            let frame = self.frame_of_at(span as f64 * self.unit_per_out());
            self.len = ((frame.round() as i64 - self.offset as i64).max(MIN_LEN as i64) as usize)
                .min(most);
            self.span = span;
            self.prune_markers();
        } else {
            self.len = ((to - self.start).max(MIN_LEN as i64) as usize).min(most);
        }
        self.fit_fades();
    }

    /// Shortens the fades, each keeping its share, so that together they
    /// fit in the clip.
    fn fit_fades(&mut self) {
        let (a, b) = (self.fade_in.len, self.fade_out.len);
        if a + b > self.len {
            let a2 = (a as f64 * self.len as f64 / (a + b) as f64).round() as usize;
            self.fade_in.len = a2.min(self.len);
            self.fade_out.len = self.len - self.fade_in.len;
        }
    }

    /// Sets a fade's length, as long as the clip leaves room for it
    /// beside the other fade.
    pub fn set_fade(&mut self, end: bool, len: usize) {
        if end {
            self.fade_out.len = len.min(self.len - self.fade_in.len);
        } else {
            self.fade_in.len = len.min(self.len - self.fade_out.len);
        }
    }

    /// Moves the audio inside the clip so source frame `offset` plays
    /// at its start, as far as the source goes; the clip stays put.
    pub fn slip_to(&mut self, offset: i64) {
        let most = (self.source.frames - self.len) as i64;
        let offset = offset.clamp(0, most);
        if self.is_stretched() {
            // The markers stay with the audio; the clip's time starts
            // afresh at the new first frame.
            let shift = self.at_of_frame(offset as f64).round() as i64;
            for m in &mut self.markers {
                m.at -= shift;
            }
            self.offset = offset as usize;
            self.prune_markers();
        } else {
            self.offset = offset as usize;
        }
    }

    /// What sets the clip's level, as the engine plays it.
    pub fn shape(&self) -> Shape {
        Shape {
            gain: db_to_gain(self.gain_db),
            fade_in: self.fade_in,
            fade_out: self.fade_out,
            envelope: self.envelope.as_slice().into(),
        }
    }

    /// The envelope's level at the clip's frame `rel` (0 dB with no
    /// points), before the clip's own volume.
    pub fn envelope_db(&self, rel: i64) -> f32 {
        envelope_db(&self.envelope, (self.offset as i64 + rel) as f64)
    }

    /// Adds a volume point at the clip's frame `rel`, or moves the one
    /// already there. Returns its index.
    pub fn add_point(&mut self, rel: i64, db: f32) -> usize {
        let at = (self.offset as i64 + rel.clamp(0, self.len as i64)) as usize;
        let db = db.clamp(MIN_DB, MAX_DB);
        match self.envelope.binary_search_by_key(&at, |p| p.at) {
            Ok(i) => {
                self.envelope[i].db = db;
                i
            }
            Err(i) => {
                self.envelope.insert(i, GainPoint { at, db });
                i
            }
        }
    }

    /// Cuts in two at timeline frame `at`; `None` if either part would be
    /// shorter than [`MIN_LEN`].
    pub fn split(&self, at: i64) -> Option<(Clip, Clip)> {
        if at - self.start < MIN_LEN as i64 || self.end() - at < MIN_LEN as i64 {
            return None;
        }
        if self.is_stretched() {
            // Cut the audio where the timeline frame plays it; each half
            // keeps the markers on its side.
            let span = (at - self.start) as usize;
            let cut_at = span as f64 * self.unit_per_out();
            let frame = self.frame_of_at(cut_at).round() as i64;
            let first = (frame - self.offset as i64)
                .clamp(MIN_LEN as i64, (self.len - MIN_LEN) as i64)
                as usize;
            let shift = self.at_of_frame((self.offset + first) as f64).round() as i64;
            let mut a = self.clone();
            a.len = first;
            a.span = span;
            a.fade_out = Fade::default();
            a.prune_markers();
            a.fit_fades();
            let mut b = self.clone();
            b.id = next_id();
            b.key = daw_project::new_id();
            b.start = at;
            b.offset += first;
            b.len -= first;
            b.span = self.span - span;
            for m in &mut b.markers {
                m.at -= shift;
            }
            if b.tempo.is_some() {
                b.beat += shift;
            }
            b.prune_markers();
            b.fade_in = Fade::default();
            b.fit_fades();
            return Some((a, b));
        }
        let first = (at - self.start) as usize;
        // The cut is clean; the fades stay at the outer ends.
        let mut a = self.clone();
        a.len = first;
        a.fade_out = Fade::default();
        a.fit_fades();
        let mut b = self.clone();
        b.id = next_id();
        b.key = daw_project::new_id();
        b.start = at;
        b.offset += first;
        b.len -= first;
        b.fade_in = Fade::default();
        b.fit_fades();
        Some((a, b))
    }

    /// Swaps in the same file opened at another rate, scaling positions
    /// to match.
    pub fn reopened(&mut self, source: Arc<StreamSource>) {
        let k = source.rate as f64 / self.source.rate as f64;
        self.start = (self.start as f64 * k).round() as i64;
        self.offset = ((self.offset as f64 * k).round() as usize).min(source.frames);
        self.len = ((self.len as f64 * k).round() as usize).min(source.frames - self.offset);
        for f in [&mut self.fade_in, &mut self.fade_out] {
            f.len = (f.len as f64 * k).round() as usize;
        }
        for p in &mut self.envelope {
            p.at = (p.at as f64 * k).round() as usize;
        }
        for m in &mut self.markers {
            m.frame = (m.frame as f64 * k).round() as i64;
            if self.tempo.is_none() {
                m.at = (m.at as f64 * k).round() as i64;
            }
        }
        self.span = (self.span as f64 * k).round() as usize;
        self.source = source;
        self.prune_markers();
        self.fit_fades();
    }

    pub fn name(&self) -> String {
        match self.take {
            Some(t) => format!("Take {}", t + 1),
            None => self.label.clone(),
        }
    }
}

/// Ranges merged where they overlap or touch, in order.
fn merged(mut r: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    r.sort_unstable();
    let mut out: Vec<(i64, i64)> = Vec::new();
    for (a, b) in r {
        match out.last_mut() {
            Some(l) if a <= l.1 => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// Ripple delete: moves `clips` earlier to close the time the `gone`
/// ranges leave empty. Time still covered by another clip stays.
pub fn close_gaps(clips: &mut [Clip], gone: &[(i64, i64)]) {
    let kept = merged(clips.iter().map(|c| (c.start, c.end())).collect());
    // The gone ranges, less what kept clips still cover.
    let mut empty = Vec::new();
    for (mut a, b) in merged(gone.to_vec()) {
        for &(ka, kb) in &kept {
            if kb <= a || ka >= b {
                continue;
            }
            if ka > a {
                empty.push((a, ka));
            }
            a = a.max(kb);
        }
        if a < b {
            empty.push((a, b));
        }
    }
    for c in clips.iter_mut() {
        let by: i64 = (empty.iter())
            .map(|&(a, b)| (b.min(c.start) - a).max(0))
            .sum();
        c.start -= by;
    }
}

/// Adds a newly recorded take to a track's clips, on top: older takes
/// under it are muted.
pub fn add_take(clips: &mut Vec<Clip>, mut c: Clip) {
    c.take = Some(take_count(clips));
    for o in clips.iter_mut() {
        if o.take.is_some() && o.overlaps(&c) {
            o.muted = true;
        }
    }
    clips.push(c);
}

/// How many takes have been recorded on the track.
pub fn take_count(clips: &[Clip]) -> usize {
    clips
        .iter()
        .filter_map(|c| c.take)
        .max()
        .map_or(0, |t| t + 1)
}

/// The newest take that plays, if any does.
pub fn active_take(clips: &[Clip]) -> Option<usize> {
    clips
        .iter()
        .filter(|c| !c.muted)
        .filter_map(|c| c.take)
        .max()
}

/// Plays take `which` where it was recorded, muting other takes under it;
/// `None` mutes every take. Imported files are left alone.
pub fn choose_take(clips: &mut [Clip], which: Option<usize>) {
    let chosen: Vec<(i64, i64)> = clips
        .iter()
        .filter(|c| which.is_some() && c.take == which)
        .map(|c| (c.start, c.end()))
        .collect();
    for c in clips.iter_mut().filter(|c| c.take.is_some()) {
        if c.take == which {
            c.muted = false;
        } else if which.is_none() || chosen.iter().any(|&(s, e)| c.start < e && s < c.end()) {
            c.muted = true;
        }
    }
}

/// The grid clips snap to and the timeline draws: bars, beats, or
/// parts of a beat, along the tempo map, so it follows tempo and time
/// signature changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Grid {
    /// The finest of bars, beats, halves, quarters and eighths of a
    /// beat that is still wide enough to see at this zoom.
    Auto,
    Bar,
    #[default]
    Beat,
    /// A beat in this many equal parts: 3 and 6 for triplets.
    Part(u8),
}

impl Grid {
    /// In the order offered.
    pub const ALL: [Grid; 8] = [
        Grid::Auto,
        Grid::Bar,
        Grid::Beat,
        Grid::Part(2),
        Grid::Part(3),
        Grid::Part(4),
        Grid::Part(6),
        Grid::Part(8),
    ];

    pub fn label(self) -> &'static str {
        match self {
            Grid::Auto => "Auto",
            Grid::Bar => "Bar",
            Grid::Beat => "Beat",
            Grid::Part(2) => "½ beat",
            Grid::Part(3) => "⅓ beat (triplets)",
            Grid::Part(4) => "¼ beat",
            Grid::Part(6) => "⅙ beat (triplets)",
            Grid::Part(8) => "⅛ beat",
            Grid::Part(_) => "Part of a beat",
        }
    }

    /// The grid's step at `at`, in ticks.
    fn ticks(self, map: &TempoMap, at: Ticks) -> i64 {
        let sig = map.signature_at(at);
        match self {
            Grid::Bar => sig.bar_ticks(),
            Grid::Part(n) => (sig.beat_ticks() / n.max(1) as i64).max(1),
            Grid::Beat | Grid::Auto => sig.beat_ticks(),
        }
    }

    /// The grid to use at `at` when `pps` points show a second: `Auto`
    /// becomes the finest step at least [`AUTO_GRID_PX`] wide.
    pub fn at_zoom(self, map: &TempoMap, at: i64, pps: f64) -> Grid {
        if self != Grid::Auto {
            return self;
        }
        let rate = map.sample_rate() as f64;
        let t = map.ticks_at(Frames(at));
        let px = |g: Grid| {
            let f0 = map.frames_at(t).0;
            let f1 = map.frames_at(Ticks(t.0 + g.ticks(map, t))).0;
            (f1 - f0) as f64 / rate * pps
        };
        [Grid::Part(8), Grid::Part(4), Grid::Part(2), Grid::Beat]
            .into_iter()
            .find(|&g| px(g) >= AUTO_GRID_PX)
            .unwrap_or(Grid::Bar)
    }
}

/// The narrowest grid step `Grid::Auto` picks, in points.
pub const AUTO_GRID_PX: f64 = 14.0;

/// `frames` moved to the nearest line of `grid` (which isn't `Auto`).
pub fn snap(map: &TempoMap, frames: i64, grid: Grid) -> i64 {
    let t = map.ticks_at(Frames(frames));
    let bb = map.bar_beat(t);
    let bar = map.bar_start(bb.bar);
    let ticks = match grid {
        Grid::Bar => {
            let next = map.bar_start(bb.bar + 1);
            if t.0 - bar.0 < next.0 - t.0 {
                bar.0
            } else {
                next.0
            }
        }
        _ => {
            let unit = grid.ticks(map, t);
            bar.0 + ((t.0 - bar.0) as f64 / unit as f64).round() as i64 * unit
        }
    };
    map.frames_at(Ticks(ticks)).0
}

/// `frames` moved by `n` steps of `grid` (which isn't `Auto`) along the
/// tempo map.
pub fn step(map: &TempoMap, frames: i64, n: i64, grid: Grid) -> i64 {
    let t = map.ticks_at(Frames(frames));
    map.frames_at(Ticks((t.0 + n * grid.ticks(map, t)).max(0)))
        .0
}

/// A position as a screen reader should say it: "bar 3", or "bar 3
/// beat 2" off the bar line.
pub fn spoken(map: &TempoMap, frames: i64) -> String {
    let bb = map.bar_beat(map.ticks_at(Frames(frames)));
    if bb.beat == 1 && bb.tick == 0 {
        format!("bar {}", bb.bar)
    } else {
        format!("bar {} beat {}", bb.bar, bb.beat)
    }
}

/// A clip's fades and volume as a screen reader should say them, after
/// its name: nothing for a clip at its plain level.
pub fn spoken_level(c: &Clip, rate: u32) -> String {
    let ms = |f: usize| (f as f64 * 1000.0 / rate as f64).round();
    let mut s = String::new();
    if c.fade_in.len > 0 {
        s += &format!(", fades in over {} ms", ms(c.fade_in.len));
    }
    if c.fade_out.len > 0 {
        s += &format!(", fades out over {} ms", ms(c.fade_out.len));
    }
    if c.gain_db.abs() >= 0.05 {
        s += &format!(", volume {:+.1} dB", c.gain_db);
    }
    if !c.envelope.is_empty() {
        s += &format!(", {} volume points", c.envelope.len());
    }
    s
}

/// A named point on a bar line: the start of a section, which runs to
/// the next marker.
#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
    pub id: String,
    /// Counted from 1.
    pub bar: i64,
    pub name: String,
}

/// Adds a marker at `bar`, keeping them in order. False if there is one
/// there already.
pub fn add_marker(markers: &mut Vec<Marker>, bar: i64) -> bool {
    let bar = bar.max(1);
    match markers.binary_search_by_key(&bar, |m| m.bar) {
        Ok(_) => false,
        Err(i) => {
            markers.insert(
                i,
                Marker {
                    id: daw_project::new_id(),
                    bar,
                    name: section_name(markers.len()),
                },
            );
            true
        }
    }
}

/// "Section A", "Section B", … for new markers, until renamed.
fn section_name(n: usize) -> String {
    let letter = (b'A' + (n % 26) as u8) as char;
    format!("Section {letter}")
}

/// The bars marker `i`'s section covers, as a loop: from its bar to the
/// next marker's, or to `end_bar` after the last.
pub fn section(markers: &[Marker], i: usize, end_bar: i64) -> (i64, i64) {
    let from = markers[i].bar;
    let to = markers.get(i + 1).map_or(end_bar, |m| m.bar);
    (from, to.max(from + 1))
}

/// One track as the timeline sees it.
pub struct Lane<'a> {
    pub name: &'a str,
    pub clips: &'a mut Vec<Clip>,
    /// Note clips (roadmap 3.1), in ticks; drawn and moved here, and
    /// edited in the note editor.
    pub notes: &'a mut Vec<NoteClip>,
    pub kind: daw_project::TrackKind,
    /// Sits in a folder: shown indented under it.
    pub in_folder: bool,
}

/// What the timeline edits: borrowed from the app for one frame.
pub struct Song<'a> {
    pub lanes: Vec<Lane<'a>>,
    pub markers: &'a mut Vec<Marker>,
}

impl Song<'_> {
    fn find_mut(&mut self, id: u64) -> Option<&mut Clip> {
        self.lanes
            .iter_mut()
            .flat_map(|l| l.clips.iter_mut())
            .find(|c| c.id == id)
    }

    fn find(&self, id: u64) -> Option<&Clip> {
        self.lanes
            .iter()
            .flat_map(|l| l.clips.iter())
            .find(|c| c.id == id)
    }

    /// Which track clip `id` is on.
    pub fn lane_of(&self, id: u64) -> Option<usize> {
        self.lanes
            .iter()
            .position(|l| l.clips.iter().any(|c| c.id == id))
    }

    /// Puts clip `id` on track `lane`, on top of that track's clips.
    /// False if it is there already or there is no such track.
    pub fn move_to_lane(&mut self, id: u64, lane: usize) -> bool {
        let Some(from) = self.lane_of(id) else {
            return false;
        };
        if from == lane || lane >= self.lanes.len() {
            return false;
        }
        let i = self.lanes[from]
            .clips
            .iter()
            .position(|c| c.id == id)
            .expect("found above");
        let c = self.lanes[from].clips.remove(i);
        self.lanes[lane].clips.push(c);
        true
    }

    /// A take that has left the track it was recorded on becomes an
    /// ordinary clip, named after the take and the track, and plays.
    fn detach_take(&mut self, id: u64, from: usize) {
        let name = self.lanes.get(from).map(|l| l.name.to_string());
        if let Some(c) = self.find_mut(id)
            && c.take.is_some()
        {
            c.label = match name {
                Some(track) => format!("{} from {track}", c.name()),
                None => c.name(),
            };
            c.take = None;
            c.muted = false;
            c.comp = false;
        }
    }

    /// Clips in Tab order: track by track, each in time order.
    fn ordered(&self) -> Vec<u64> {
        let mut out = Vec::new();
        for l in &self.lanes {
            let mut ids: Vec<(i64, u64)> = l.clips.iter().map(|c| (c.start, c.id)).collect();
            ids.sort_unstable();
            out.extend(ids.into_iter().map(|(_, id)| id));
        }
        out
    }

    /// The clip on track `lane` that starts nearest to `at`.
    fn nearest_on(&self, lane: usize, at: i64) -> Option<u64> {
        self.lanes
            .get(lane)?
            .clips
            .iter()
            .min_by_key(|c| ((c.start - at).abs(), c.id))
            .map(|c| c.id)
    }

    /// The end of the last clip, in frames.
    pub fn end(&self) -> i64 {
        self.lanes
            .iter()
            .flat_map(|l| l.clips.iter())
            .map(Clip::end)
            .max()
            .unwrap_or(0)
    }
}

/// A comment thread's pin on the timeline.
pub struct Pin {
    pub id: String,
    /// The lane it is on, or `None` for the whole song.
    pub lane: Option<usize>,
    pub start: i64,
    pub end: Option<i64>,
    pub resolved: bool,
    pub selected: bool,
    /// The start of its text, shown on hover.
    pub text: String,
}

/// Where things are, for one frame.
pub struct View<'a> {
    pub map: &'a TempoMap,
    pub rate: u32,
    pub playhead: i64,
    pub playing: bool,
    pub loop_bars: Option<(i64, i64)>,
    pub can_undo: bool,
    pub can_redo: bool,
    pub pins: &'a [Pin],
    /// Commands for the timeline, run since it was last drawn.
    pub commands: &'a [Cmd],
    /// The picture's timecode, if shown.
    pub timecode: Option<daw_engine::timecode::Clock>,
    pub hits: &'a [daw_project::HitPoint],
    /// Each lane's track: whether it can be frozen, and whether it is.
    pub freeze: &'a [(bool, bool)],
    /// Each lane's track versions: their names and the place of the one
    /// playing; no names for a track without versions.
    pub versions: &'a [(Vec<String>, usize)],
    /// Clips have been copied.
    pub can_paste: bool,
    /// Each lane's notation band under its row, if it shows one.
    pub bands: &'a [Option<crate::notation::Band<'a>>],
    /// Each lane's header: its mute, solo and arm, and its meter.
    pub heads: &'a [Head],
    /// The lane shown in the inspector, marked on its header.
    pub inspected: Option<usize>,
}

/// What a track header shows besides the name.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Head {
    pub mute: bool,
    pub solo: bool,
    /// Soloing others leaves it heard.
    pub solo_safe: bool,
    /// Turned off: drawn faded.
    pub off: bool,
    /// Armed for recording; `None` for a track that can't record (a
    /// bus, a folder, or an audio track with no input).
    pub arm: Option<bool>,
    /// Peak levels, 0 to 1 and over, left and right.
    pub meter: (f32, f32),
}

/// Something asked of a track from its header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeadAsk {
    /// Show it in the inspector.
    Select,
    Mute,
    Solo,
    Arm,
}

/// Something asked of a track from its menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackAsk {
    /// Open its instrument.
    Instrument,
    /// Open its effects.
    Effects,
    /// Freeze it, or unfreeze it.
    Freeze,
    /// Turn it off, or on again.
    Off,
    /// Make it solo-safe, or not.
    SoloSafe,
    RenderInPlace,
    /// Save it as a track preset.
    SavePreset,
    /// Something to do with its versions.
    Version(crate::versions::VersionAsk),
}

/// What the app should do after the timeline is drawn.
#[derive(Default)]
pub struct Outcome {
    /// Clips changed: rebuild the graph.
    pub changed: bool,
    pub seek: Option<i64>,
    /// Loop these bars.
    pub loop_bars: Option<(i64, i64)>,
    pub undo: bool,
    pub redo: bool,
    /// A comment's pin was clicked.
    pub pin: Option<String>,
    /// Write a comment here: on a lane (or the whole song) at a frame.
    pub comment_at: Option<(Option<usize>, i64)>,
    /// Write tab for this marker's section.
    pub tab_for: Option<String>,
    /// Add a tempo or time-signature change, or open their window.
    pub tempo: Option<crate::tempo::Ask>,
    /// Process the selected clips into new audio.
    pub process: Option<Process>,
    /// Add or move a hit point, or open their window.
    pub hit: Option<crate::hitpoints::HitAsk>,
    /// Freeze or render the track on this lane.
    pub track: Option<(usize, TrackAsk)>,
    /// Bounce the selected clips to new tracks.
    pub bounce: bool,
    /// Glue the selected clips into one (or the one picked to the next).
    pub glue: bool,
    /// Something to tell the user.
    pub message: Option<String>,
    /// Copy the selected clips.
    pub copy: bool,
    /// Paste the copied clips onto a lane at a frame: as they are, or
    /// (true) rendered through their tracks' effects.
    pub paste: Option<(usize, i64, bool)>,
    /// Open the note editor on this note clip, by key.
    pub edit_notes: Option<String>,
    /// Open the pitch editor on the audio clip now selected (a
    /// double-click on it).
    pub edit_audio: bool,
    /// Where the band asked for with [`Timeline::band`] is, track names'
    /// column and all, for the app to draw in.
    pub band: Option<Rect>,
    /// Add, edit or copy a drum pattern (roadmap 3.3).
    pub pattern: Option<PatternAsk>,
    /// A lane's notation band was clicked: the lane, the tick under the
    /// pointer, and the height into the band.
    pub notation_band: Option<(usize, i64, f32)>,
    /// Somewhere else on the lanes was clicked.
    pub off_band: bool,
    /// Run this command on the selection.
    pub command: Option<Cmd>,
    /// A track header was clicked, or one of its buttons.
    pub head: Option<(usize, HeadAsk)>,
}

/// What the timeline asks of patterns, which the app keeps.
#[derive(Clone, Debug, PartialEq)]
pub enum PatternAsk {
    /// A new pattern, and a clip playing it on this lane at this frame.
    Add(usize, i64),
    /// Open the pattern editor on the pattern this note clip plays.
    Edit(String),
    /// Give this note clip a copy of its pattern of its own.
    Unique(String),
    /// Make this note clip's notes its own, no longer following its
    /// pattern.
    ToNotes(String),
}

/// The amounts, in dB, the clip menu's Change gain offers.
const GAIN_STEPS: [f32; 6] = [6.0, 3.0, 1.0, -1.0, -3.0, -6.0];

/// Frames between waveform peaks at the finest level.
const BASE_HOP: usize = 256;

/// A file's peaks at successively halved resolutions.
pub(crate) struct Waveform {
    levels: Vec<Vec<(f32, f32)>>,
}

impl Waveform {
    fn new(base: Vec<(f32, f32)>) -> Waveform {
        let mut levels = vec![base];
        while levels.last().is_some_and(|l| l.len() > 1) {
            let next = levels[levels.len() - 1]
                .chunks(2)
                .map(|c| {
                    c.iter()
                        .fold((0.0f32, 0.0f32), |(lo, hi), &(a, b)| (lo.min(a), hi.max(b)))
                })
                .collect();
            levels.push(next);
        }
        Waveform { levels }
    }

    /// Lowest and highest sample in source frames `from..to`.
    pub(crate) fn range(&self, from: usize, to: usize) -> (f32, f32) {
        let span = to.saturating_sub(from).max(1);
        // The coarsest level still finer than the span.
        let mut l = 0;
        while l + 1 < self.levels.len() && BASE_HOP << (l + 1) <= span {
            l += 1;
        }
        let hop = BASE_HOP << l;
        let level = &self.levels[l];
        let (i0, i1) = (from / hop, to.div_ceil(hop).max(from / hop + 1));
        level[i0.min(level.len())..i1.min(level.len())]
            .iter()
            .fold((0.0, 0.0), |(lo, hi), &(a, b)| (lo.min(a), hi.max(b)))
    }
}

/// Waveforms by streamed file, worked out in the background.
#[derive(Default)]
struct Waveforms {
    ready: HashMap<PathBuf, Arc<Waveform>>,
    pending: HashMap<PathBuf, mpsc::Receiver<Option<Waveform>>>,
}

impl Waveforms {
    fn get(&mut self, src: &Arc<StreamSource>) -> Option<Arc<Waveform>> {
        if let Some(w) = self.ready.get(&src.path) {
            return Some(w.clone());
        }
        if !self.pending.contains_key(&src.path) {
            let (tx, rx) = mpsc::channel();
            let s = src.clone();
            std::thread::spawn(move || {
                let _ = tx.send(s.peaks(BASE_HOP).ok().map(Waveform::new));
            });
            self.pending.insert(src.path.clone(), rx);
        }
        None
    }

    /// Takes in finished waveforms. True while any are still coming.
    fn poll(&mut self) -> bool {
        let mut done = Vec::new();
        for (path, rx) in &self.pending {
            match rx.try_recv() {
                Ok(w) => done.push((path.clone(), w)),
                Err(mpsc::TryRecvError::Disconnected) => done.push((path.clone(), None)),
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        for (path, w) in done {
            self.pending.remove(&path);
            // A file that can't be read draws as a plain box.
            let w = w.unwrap_or_else(|| Waveform::new(Vec::new()));
            self.ready.insert(path, Arc::new(w));
        }
        !self.pending.is_empty()
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Zone {
    Body,
    StartEdge,
    EndEdge,
    /// The handle at the top of a fade.
    FadeIn,
    FadeOut,
    /// The volume line, or one of its points.
    Volume(Option<usize>),
    /// A stretch marker, by index.
    Marker(usize),
}

#[derive(Clone, Copy)]
enum Hit {
    Ruler,
    Marker(usize),
    Pin(usize),
    Clip {
        id: u64,
        zone: Zone,
    },
    /// A note clip: which lane, which of its note clips, and whether
    /// its end edge (to trim) rather than its body.
    NoteClip {
        lane: usize,
        index: usize,
        edge: bool,
    },
    /// The notation band under a lane's row.
    Band(usize),
    Lane,
    /// A track's header, left of its lane: right-clicked only.
    Header(usize),
}

/// A note clip being moved or trimmed.
struct NoteDrag {
    lane: usize,
    key: String,
    /// Timeline frame under the pointer when pressed.
    from: i64,
    /// Its start and length when pressed, in ticks.
    start: i64,
    len: i64,
    edge: bool,
    moved: bool,
}

/// The shortest a note clip can be trimmed to: a sixteenth note.
const MIN_NOTE_CLIP: i64 = PPQ / 4;

/// A new note clip a bar long from `at`, the `n`th made.
fn new_note_clip(map: &TempoMap, at: i64, n: usize) -> NoteClip {
    let start = map.ticks_at(Frames(at.max(0)));
    let bar = map.signature_at(start).bar_ticks();
    NoteClip::new(daw_project::new_id(), format!("Notes {n}"), start.0, bar)
}

struct Drag {
    zone: Zone,
    /// The clip held.
    anchor: u64,
    /// Timeline frame under the pointer when pressed.
    from: i64,
    /// The track under the pointer when pressed.
    from_lane: usize,
    /// The clips being dragged, as they were, with their tracks.
    originals: Vec<(usize, Clip)>,
    /// Moving the audio inside the clips rather than the clips.
    slip: bool,
    /// Ripple: the clips after a trimmed end, and where they started.
    after: Vec<(u64, i64)>,
    /// Height under the pointer when pressed.
    from_y: f32,
    /// Grouped clips are held: they stay on their tracks.
    grouped: bool,
    moved: bool,
}

/// What has the keyboard on the timeline: a clip, or a marker by its
/// id in the project document.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Focus {
    Clip(u64),
    Marker(String),
}

impl Focus {
    /// The widget id of the item's entry in the accessibility tree.
    fn id(&self, ui: &egui::Ui) -> egui::Id {
        match self {
            Focus::Clip(c) => ui.scope_id().with(("clip", c)),
            Focus::Marker(m) => ui.scope_id().with(("marker", m)),
        }
    }
}

pub struct Timeline {
    /// Zoom.
    px_per_sec: f32,
    /// Seconds at the left edge of the lanes.
    scroll: f64,
    /// Points scrolled down the lanes.
    vscroll: f32,
    /// The grid drawn, and snapped to when `snap` is on.
    pub grid: Grid,
    pub snap: bool,
    /// Snap to other clips' edges, markers and the playhead when one is
    /// near.
    pub magnet: bool,
    /// Deleting a clip, or trimming its end, moves the clips after it
    /// on its track to close (or open) the gap.
    pub ripple: bool,
    /// Show each clip's volume line, to drag and add points to.
    pub show_volume: bool,
    /// While playing, scroll the lanes along under a still playhead
    /// rather than a page at a time.
    pub continuous: bool,
    /// The project's active edit groups, set by the app each frame.
    pub groups: Vec<LaneGroup>,
    /// Edit grouped tracks together; off, each clip is edited alone.
    pub group_edit: bool,
    /// The simpler mode is on: the advanced controls are left out.
    pub simple: bool,
    /// The fades and volume window for the selected clips is open.
    pub fades_open: bool,
    /// Width of the lanes last drawn, in points, for zooming.
    width: f32,
    /// A value in the fades window changed while being dragged.
    fades_unsettled: bool,
    pub selected: HashSet<u64>,
    drag: Option<Drag>,
    /// The selected note clip, by key, and one being dragged.
    pub note_sel: Option<String>,
    note_drag: Option<NoteDrag>,
    /// Scrubbing the playhead along the ruler.
    scrubbing: bool,
    /// Dragging one end of the loop along the ruler: 0 its start, 1 its
    /// end.
    loop_drag: Option<usize>,
    waves: Waveforms,
    /// The words found in each recording (crate::lyrics), by its file.
    pub words: HashMap<PathBuf, Arc<daw_speech::Transcript>>,
    /// What was right-clicked, and where (timeline frame).
    menu: Option<(Hit, i64)>,
    /// The lane right-clicked, or `None` for the ruler.
    menu_lane: Option<usize>,
    /// A marker being renamed: its index and the text so far.
    rename: Option<(usize, String)>,
    /// The rename field has been given the keyboard.
    rename_focused: bool,
    /// The clip or marker with keyboard focus, and its widget id.
    focus: Option<(Focus, egui::Id)>,
    /// Give the keyboard to this item on the next frame.
    focus_next: Option<Focus>,
    /// The hit point being dragged along the timecode strip, and where
    /// the strip was right-clicked.
    hit_drag: Option<String>,
    strip_at: i64,
    /// The height of a band between the ruler and the tracks, kept for
    /// the app to draw in (the video filmstrip), or 0 for none. Set by
    /// the app each frame.
    pub band: f32,
}

impl Timeline {
    /// Seconds at the left edge of the lanes, and points per second,
    /// for drawing in line with them.
    pub(crate) fn x_view(&self) -> (f64, f32) {
        (self.scroll, self.px_per_sec)
    }

    /// A file's waveform, once worked out.
    pub(crate) fn wave(&mut self, src: &Arc<StreamSource>) -> Option<Arc<Waveform>> {
        self.waves.get(src)
    }
}

impl Default for Timeline {
    fn default() -> Timeline {
        Timeline {
            px_per_sec: 40.0,
            scroll: 0.0,
            vscroll: 0.0,
            grid: Grid::default(),
            snap: true,
            magnet: false,
            ripple: false,
            show_volume: false,
            continuous: false,
            simple: false,
            groups: Vec::new(),
            group_edit: true,
            fades_open: false,
            width: 600.0,
            fades_unsettled: false,
            selected: HashSet::new(),
            drag: None,
            scrubbing: false,
            loop_drag: None,
            waves: Waveforms::default(),
            words: HashMap::new(),
            menu: None,
            menu_lane: None,
            note_sel: None,
            note_drag: None,
            rename: None,
            rename_focused: false,
            focus: None,
            focus_next: None,
            hit_drag: None,
            strip_at: 0,
            band: 0.0,
        }
    }
}

/// A header's first row (the name's), and where its buttons go: beside
/// the name in a short lane, on a row of their own under it in a tall
/// one.
fn header_rows(r: Rect, head: Head) -> (Rect, Option<Rect>) {
    let n = if head.arm.is_some() { 3.0 } else { 2.0 };
    let w = n * HEAD_BUTTON.x + (n - 1.0) * HEAD_GAP;
    let right = r.right() - HEAD_METER - 10.0;
    if r.height() >= HEAD_TWO_ROWS {
        let row = Rect::from_x_y_ranges(r.x_range(), r.top() + 4.0..=r.top() + 24.0);
        let b = Rect::from_min_size(
            pos2(r.left() + 16.0, r.bottom() - HEAD_BUTTON.y - 6.0),
            vec2(w, HEAD_BUTTON.y),
        );
        (row, Some(b))
    } else {
        let b = Rect::from_min_size(
            pos2(right - w, r.center().y - HEAD_BUTTON.y / 2.0),
            vec2(w, HEAD_BUTTON.y),
        );
        (r, Some(b))
    }
}

/// Track headers to the left of the lanes: colour, kind, name, mute,
/// solo, arm and a level meter.
pub(crate) const HEADER: f32 = 176.0;
/// A header button's size, and the gap between them.
const HEAD_BUTTON: Vec2 = vec2(20.0, 18.0);
const HEAD_GAP: f32 = 3.0;
/// The header's level meter: its width, inside the right edge.
const HEAD_METER: f32 = 6.0;
/// A lane this tall or taller shows its buttons on a second row.
const HEAD_TWO_ROWS: f32 = 50.0;
/// The ruler: marker flags on top, then tempo and time-signature
/// changes, then the loop and bar numbers.
pub(crate) const RULER: f32 = 38.0;
/// Edge grab width for trimming, in points.
const EDGE: f32 = 6.0;
/// A fade handle's size, and how near counts as on the volume line.
const HANDLE: f32 = 8.0;
/// How near another clip's edge or a marker must be to snap to it, in
/// points.
const MAGNET: f32 = 8.0;

/// Height in a clip's box `r` of a level in dB.
fn db_y(r: Rect, db: f32) -> f32 {
    let k = (MAX_DB - db.clamp(MIN_DB, MAX_DB)) / (MAX_DB - MIN_DB);
    r.top() + k * r.height()
}

/// The level in dB at height `y` in a clip's box `r`.
fn y_db(r: Rect, y: f32) -> f32 {
    let k = ((y - r.top()) / r.height().max(1.0)).clamp(0.0, 1.0);
    MAX_DB - k * (MAX_DB - MIN_DB)
}
/// A comment pin's bubble.
const PIN: egui::Vec2 = vec2(14.0, 11.0);

/// A pin's bubble, ending at `x` with its top at `top`.
fn pin_rect(x: f32, top: f32) -> Rect {
    Rect::from_min_size(pos2(x - PIN.x, top + 1.0), PIN - vec2(0.0, 1.0))
}

/// A clip's volume line in its box `r`, with its points.
/// The stretch markers on a clip: a line down the clip with a flag at
/// the top, where each tied frame plays.
fn draw_markers(
    painter: &egui::Painter,
    look: &Look,
    c: &Clip,
    map: &TempoMap,
    x_of: &dyn Fn(i64) -> f32,
    colour: Color32,
) {
    let r = painter.clip_rect();
    let stroke = Stroke::new(look.line, colour.gamma_multiply(0.9));
    for m in &c.markers {
        let x = x_of(c.out_of_frame(map, m.frame as f64));
        painter.line_segment([pos2(x, r.top()), pos2(x, r.bottom())], stroke);
        painter.add(egui::Shape::convex_polygon(
            vec![
                pos2(x - HANDLE / 2.0, r.top()),
                pos2(x + HANDLE / 2.0, r.top()),
                pos2(x, r.top() + HANDLE * 0.8),
            ],
            colour,
            Stroke::NONE,
        ));
    }
}

/// The words found in a clip's recording, along its bottom edge where
/// each is sung; a word that would run into the one before is left out
/// until zoomed in.
fn draw_words(
    painter: &egui::Painter,
    look: &Look,
    c: &Clip,
    t: &daw_speech::Transcript,
    map: &TempoMap,
    x_of: &dyn Fn(i64) -> f32,
    ink: Color32,
) {
    let r = painter.clip_rect();
    let rate = c.source.rate as f64;
    let (lo, hi) = (c.offset as f64, (c.offset + c.len) as f64);
    let font = look.font(look.small);
    let mut free = f32::MIN;
    for w in t.words() {
        let f = w.start * rate;
        if f < lo || f >= hi {
            continue;
        }
        let x = x_of(c.out_of_frame(map, f));
        if x < free || x > r.right() {
            continue;
        }
        let g = painter.layout_no_wrap(w.text.clone(), font.clone(), ink);
        let width = g.size().x;
        if x + width < r.left() {
            continue;
        }
        painter.galley(pos2(x + 1.0, r.bottom() - g.size().y - 1.0), g, ink);
        free = x + width + 6.0;
    }
}

fn draw_volume(painter: &egui::Painter, look: &Look, c: &Clip, r: Rect, x_of: &dyn Fn(i64) -> f32) {
    let colour = look.c().accent;
    let w = (r.width().max(1.0) / 2.0).ceil() as usize;
    let line: Vec<Pos2> = (0..=w)
        .map(|i| {
            let k = i as f32 / w as f32;
            let rel = (c.len as f64 * k as f64) as i64;
            pos2(
                r.left() + k * r.width(),
                db_y(r, c.gain_db + c.envelope_db(rel)),
            )
        })
        .collect();
    painter.add(egui::Shape::line(
        line,
        Stroke::new(1.5 * look.line, colour),
    ));
    for p in &c.envelope {
        let rel = p.at as i64 - c.offset as i64;
        if (0..=c.len as i64).contains(&rel) {
            let at = pos2(x_of(c.start + rel), db_y(r, c.gain_db + p.db));
            painter.circle_filled(at, 3.5, colour);
            painter.circle_stroke(at, 3.5, Stroke::new(1.0, look.c().lane));
        }
    }
}

/// A clip's waveform, from its columns of `(x, top, bottom)`, in the
/// theme's style.
pub(crate) fn draw_wave(
    painter: &egui::Painter,
    look: &Look,
    cols: &[(f32, f32, f32)],
    mid: f32,
    c: Color32,
) {
    let w = look.wave_weight;
    // One line swinging between the top and bottom, two pixels a step.
    let swing = || -> Vec<Pos2> {
        (cols.iter().step_by(2).enumerate())
            .map(|(i, &(x, top, bottom))| pos2(x, if i % 2 == 0 { top } else { bottom }))
            .collect()
    };
    match look.wave {
        WaveStyle::Filled => {
            for &(x, top, bottom) in cols {
                painter.line_segment([pos2(x, top), pos2(x, bottom)], Stroke::new(1.0, c));
            }
        }
        WaveStyle::Outline => {
            let tops = cols.iter().map(|&(x, t, _)| pos2(x, t)).collect();
            let bottoms = cols.iter().map(|&(x, _, b)| pos2(x, b)).collect();
            painter.add(egui::Shape::line(tops, Stroke::new(w, c)));
            painter.add(egui::Shape::line(bottoms, Stroke::new(w, c)));
            if let (Some(a), Some(b)) = (cols.first(), cols.last()) {
                painter.line_segment(
                    [pos2(a.0, mid), pos2(b.0, mid)],
                    Stroke::new(1.0, c.gamma_multiply(0.4)),
                );
            }
        }
        WaveStyle::Line => {
            painter.add(egui::Shape::line(swing(), Stroke::new(w, c)));
        }
        WaveStyle::Trace => {
            let pts = swing();
            let glow = look.glow.unwrap_or(4.0);
            painter.add(egui::Shape::line(
                pts.clone(),
                Stroke::new(w + glow, c.gamma_multiply(0.18)),
            ));
            painter.add(egui::Shape::line(pts, Stroke::new(w, c)));
        }
    }
}

/// Paper grain: a faint, fixed scatter of specks over `rect`.
fn draw_grain(painter: &egui::Painter, rect: Rect, c: Color32) {
    const CELL: i32 = 5;
    let (x0, y0) = (rect.left() as i32 / CELL, rect.top() as i32 / CELL);
    let (x1, y1) = (
        rect.right() as i32 / CELL + 1,
        rect.bottom() as i32 / CELL + 1,
    );
    for gy in y0..y1 {
        for gx in x0..x1 {
            let h = crate::theme::hash(gx, gy);
            if !h.is_multiple_of(3) {
                continue;
            }
            let at = pos2(
                (gx * CELL) as f32 + (h >> 8) as f32 % CELL as f32,
                (gy * CELL) as f32 + (h >> 16) as f32 % CELL as f32,
            );
            painter.rect_filled(Rect::from_min_size(at, vec2(1.0, 1.0)), 0.0, c);
        }
    }
}

/// A fade shape's name, as offered.
pub fn curve_label(c: Curve) -> &'static str {
    match c {
        Curve::Linear => "Straight",
        Curve::EqualPower => "Smooth (steady power)",
        Curve::SCurve => "S-curve",
        Curve::Fast => "Fast start",
        Curve::Slow => "Slow start",
    }
}

/// Which part of clip `c`, drawn in `r`, is at `pos`.
fn clip_zone(
    c: &Clip,
    map: &TempoMap,
    pos: Pos2,
    r: Rect,
    x_of: &dyn Fn(i64) -> f32,
    volume: bool,
) -> Zone {
    let (x0, x1) = (r.left(), r.right());
    for (i, m) in c.markers.iter().enumerate() {
        let x = x_of(c.out_of_frame(map, m.frame as f64));
        if (pos.x - x).abs() <= HANDLE / 2.0 + 1.0 && x > x0 + EDGE && x < x1 - EDGE {
            return Zone::Marker(i);
        }
    }
    if pos.y < r.top() + HANDLE && x1 - x0 > 3.0 * HANDLE {
        let fin = x_of(c.start + c.fade_in.len as i64).max(x0 + HANDLE / 2.0);
        let fout = x_of(c.end() - c.fade_out.len as i64).min(x1 - HANDLE / 2.0);
        if (pos.x - fin).abs() <= HANDLE / 2.0 + 1.0 {
            return Zone::FadeIn;
        }
        if (pos.x - fout).abs() <= HANDLE / 2.0 + 1.0 {
            return Zone::FadeOut;
        }
    }
    let edge = EDGE.min((x1 - x0) / 3.0);
    if pos.x - x0 < edge {
        return Zone::StartEdge;
    }
    if x1 - pos.x < edge {
        return Zone::EndEdge;
    }
    if volume {
        for (i, p) in c.envelope.iter().enumerate() {
            let rel = p.at as i64 - c.offset as i64;
            if rel < 0 || rel > c.len as i64 {
                continue;
            }
            let at = pos2(x_of(c.start + rel), db_y(r, c.gain_db + p.db));
            if at.distance(pos) <= HANDLE / 2.0 + 1.0 {
                return Zone::Volume(Some(i));
            }
        }
        let rel = (c.len as f64 * ((pos.x - x0) / (x1 - x0).max(1.0)) as f64) as i64;
        let y = db_y(r, c.gain_db + c.envelope_db(rel));
        if (pos.y - y).abs() <= HANDLE / 2.0 {
            return Zone::Volume(None);
        }
    }
    Zone::Body
}

impl Timeline {
    /// Splits the selected clips at `at`, or, with none selected, every
    /// playing clip there. True if any were split.
    pub fn split_at(&mut self, song: &mut Song, at: i64) -> bool {
        let mut any = false;
        let all = self.selected.is_empty();
        for lane in &mut song.lanes {
            let mut i = 0;
            while i < lane.clips.len() {
                let c = &lane.clips[i];
                let wanted = if all {
                    !c.muted
                } else {
                    self.selected.contains(&c.id)
                };
                if wanted && let Some((a, b)) = c.split(at) {
                    if self.selected.contains(&a.id) {
                        self.selected.insert(b.id);
                    }
                    lane.clips[i] = a;
                    lane.clips.insert(i + 1, b);
                    any = true;
                    i += 1;
                }
                i += 1;
            }
        }
        any
    }

    /// Removes the selected clips. True if any were.
    pub fn delete_selected(&mut self, song: &mut Song) -> bool {
        if self.selected.is_empty() {
            return false;
        }
        let mut any = false;
        for lane in &mut song.lanes {
            let n = lane.clips.len();
            let gone: Vec<(i64, i64)> = (lane.clips.iter())
                .filter(|c| self.selected.contains(&c.id))
                .map(|c| (c.start, c.end()))
                .collect();
            lane.clips.retain(|c| !self.selected.contains(&c.id));
            any |= lane.clips.len() != n;
            if self.ripple {
                close_gaps(lane.clips, &gone);
            }
        }
        self.selected.clear();
        any
    }

    /// A clip's fades in its box `r`: the faded part shaded above its
    /// curve, and a handle at the top of each to drag.
    #[allow(clippy::too_many_arguments)]
    fn draw_fades(
        &self,
        painter: &egui::Painter,
        c: &Clip,
        r: Rect,
        x_of: &dyn Fn(i64) -> f32,
        look: &Look,
        colour: Color32,
        handles: bool,
    ) {
        // The faded part is washed out towards the lane's colour.
        let shade = look.c().lane.gamma_multiply(0.6);
        let line = Stroke::new(1.5 * look.line, colour);
        for (end, f) in [(false, c.fade_in), (true, c.fade_out)] {
            if f.len > 0 {
                let (a, b) = if end {
                    (x_of(c.end() - f.len as i64), r.right())
                } else {
                    (r.left(), x_of(c.start + f.len as i64))
                };
                let span = (b - a).max(1.0);
                let n = span.ceil() as usize;
                let mut pts = Vec::with_capacity(n + 1);
                for i in 0..=n {
                    let x = a + span * i as f32 / n as f32;
                    let k = (x - a) / span;
                    let g = f.curve.gain(if end { 1.0 - k } else { k });
                    let y = r.bottom() - g * r.height();
                    if i < n {
                        painter.line_segment([pos2(x + 0.5, r.top()), pos2(x + 0.5, y)], {
                            Stroke::new(1.0, shade)
                        });
                    }
                    pts.push(pos2(x, y));
                }
                painter.add(egui::Shape::line(pts, line));
            }
            // The handle sits at the fade's inner end, at the top.
            let x = if end {
                x_of(c.end() - f.len as i64).min(r.right() - HANDLE / 2.0)
            } else {
                x_of(c.start + f.len as i64).max(r.left() + HANDLE / 2.0)
            };
            if handles && r.width() > 3.0 * HANDLE {
                let h = Rect::from_center_size(
                    pos2(x, r.top() + HANDLE / 2.0),
                    vec2(HANDLE - 1.0, HANDLE - 1.0),
                );
                painter.rect_filled(h, 1.0, look.c().raised);
                painter.rect_stroke(h, 1.0, Stroke::new(1.0, look.c().text), StrokeKind::Inside);
            }
        }
    }

    /// The fades and volume of the selected clips, to set by number:
    /// what the first one has, and a change goes to them all.
    fn fades_window(
        &mut self,
        ctx: &egui::Context,
        song: &mut Song,
        view: &View,
        out: &mut Outcome,
    ) {
        let ids: Vec<u64> = (song.ordered().into_iter())
            .filter(|id| self.selected.contains(id))
            .collect();
        let Some(first) = ids.first().and_then(|&id| song.find(id)).cloned() else {
            self.fades_open = false;
            return;
        };
        let ms = |f: usize| f as f64 * 1000.0 / view.rate as f64;
        let frames = |ms: f64| (ms * view.rate as f64 / 1000.0).round().max(0.0) as usize;
        let longest = ids
            .iter()
            .filter_map(|&id| song.find(id))
            .map(|c| c.len)
            .max()
            .unwrap_or(0);
        let mut open = true;
        let mut fade = [first.fade_in, first.fade_out];
        let mut fade_ms = [ms(first.fade_in.len), ms(first.fade_out.len)];
        let mut gain = first.gain_db;
        let (mut set_len, mut set_curve, mut set_gain) = ([false; 2], [false; 2], false);
        let mut clear = false;
        let mut cross = false;
        // A value being dragged changes as it goes; the graph is
        // rebuilt once it is let go.
        let mut settled = true;
        egui::Window::new("Fades and volume")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(if ids.len() == 1 {
                    first.name()
                } else {
                    format!("{} clips: changes go to them all", ids.len())
                });
                egui::Grid::new("fades").num_columns(3).show(ui, |ui| {
                    for (i, name) in ["Fade in", "Fade out"].into_iter().enumerate() {
                        ui.label(name);
                        let r = ui
                            .add(
                                // ai: edit_clip.fade_in_seconds
                                egui::DragValue::new(&mut fade_ms[i])
                                    .range(0.0..=ms(longest))
                                    .speed(1.0)
                                    .max_decimals(0)
                                    .suffix(" ms"),
                            )
                            .accessible_name(format!("{name} length"));
                        set_len[i] = r.changed();
                        settled &= !r.dragged();
                        let before = fade[i].curve;
                        egui::ComboBox::from_id_salt(("curve", i))
                            .selected_text(curve_label(fade[i].curve))
                            .show_ui(ui, |ui| {
                                for c in Curve::ALL {
                                    // ai: edit_clip_volume.fade_in_curve
                                    ui.selectable_value(&mut fade[i].curve, c, curve_label(c));
                                }
                            })
                            .response
                            .accessible_name(format!("{name} shape"));
                        set_curve[i] = fade[i].curve != before;
                        ui.end_row();
                    }
                    ui.label("Volume");
                    let r = ui
                        .add(
                            // ai: edit_clip.gain_db
                            egui::Slider::new(&mut gain, MIN_DB..=MAX_DB)
                                .suffix(" dB")
                                .max_decimals(1),
                        )
                        .accessible_name("Clip volume");
                    set_gain = r.changed();
                    settled &= !r.dragged();
                    // ai: edit_clip.gain_db
                    if ui.button("0 dB").clicked() {
                        gain = 0.0;
                        set_gain = true;
                    }
                    ui.end_row();
                    let points = first.envelope.len();
                    ui.label("Volume points");
                    ui.label(points.to_string());
                    clear = ui
                        // ai: edit_clip_volume.clear_points
                        .add_enabled(points > 0, egui::Button::new("Remove all"))
                        .clicked();
                    ui.end_row();
                });
                ui.weak("Drag a fade's square handle at the top of a clip, or turn on Volume in the toolbar to draw a volume line.");
                cross = ui
                    // ai: crossfade
                    .add_enabled(ids.len() > 1, egui::Button::new("Crossfade where they meet"))
                    .on_hover_text("Select clips next to each other on a track first")
                    .clicked();
            });
        let mut changed = false;
        for &id in &ids {
            let Some(c) = song.find_mut(id) else { continue };
            for (i, end) in [(0, false), (1, true)] {
                let f = if end { &mut c.fade_out } else { &mut c.fade_in };
                if set_curve[i] {
                    changed |= f.curve != fade[i].curve;
                    f.curve = fade[i].curve;
                }
                if set_len[i] {
                    c.set_fade(end, frames(fade_ms[i]));
                    changed = true;
                }
            }
            if set_gain {
                c.gain_db = gain;
                changed = true;
            }
            if clear && !c.envelope.is_empty() {
                c.envelope.clear();
                changed = true;
            }
        }
        if cross {
            changed |= self.crossfade_selected(song, view.rate);
        }
        self.fades_unsettled |= changed && !settled;
        if settled && (changed || self.fades_unsettled) {
            self.fades_unsettled = false;
            out.changed = true;
        }
        self.fades_open = open;
    }

    /// Gives the selected clips' fades in (or out) shape `c`. True if
    /// any changed.
    fn set_curves(&mut self, song: &mut Song, end: bool, c: Curve) -> bool {
        let mut any = false;
        for clip in (song.lanes.iter_mut())
            .flat_map(|l| l.clips.iter_mut())
            .filter(|clip| self.selected.contains(&clip.id))
        {
            let f = if end {
                &mut clip.fade_out
            } else {
                &mut clip.fade_in
            };
            any |= f.curve != c;
            f.curve = c;
        }
        any
    }

    /// Fades the selected clips into each other where they meet on a
    /// track, or, with none selected, every pair that meets. True if any
    /// did.
    pub fn crossfade_selected(&mut self, song: &mut Song, rate: u32) -> bool {
        let len = (CROSSFADE_SECONDS * rate as f64) as usize;
        let all = self.selected.is_empty();
        let mut any = false;
        for lane in &mut song.lanes {
            let mut order: Vec<usize> = (0..lane.clips.len())
                .filter(|&i| !lane.clips[i].muted)
                .filter(|&i| all || self.selected.contains(&lane.clips[i].id))
                .collect();
            order.sort_by_key(|&i| (lane.clips[i].start, lane.clips[i].id));
            for w in order.windows(2) {
                let (i, j) = (w[0].min(w[1]), w[0].max(w[1]));
                let (head, tail) = lane.clips.split_at_mut(j);
                any |= crossfade(&mut head[i], &mut tail[0], len);
            }
        }
        any
    }

    pub fn add_marker(&mut self, song: &mut Song, bar: i64) {
        add_marker(song.markers, bar);
    }

    /// A track's header: its colour, kind, name, the marks of its edit
    /// group and version, and its level meter. Its buttons are drawn
    /// over it afterwards by [`Timeline::header_buttons`].
    #[allow(clippy::too_many_arguments)]
    fn paint_header(
        &self,
        painter: &egui::Painter,
        r: Rect,
        i: usize,
        lane: &Lane,
        head: Head,
        view: &View,
        look: &Look,
    ) {
        let tc = look.c();
        let inspected = view.inspected == Some(i);
        painter.rect_filled(r, 0.0, if inspected { tc.raised } else { tc.panel });
        let colour = tc.track(i);
        // The track's colour as a tag, as in the mixer.
        painter.rect_filled(
            Rect::from_min_size(r.min + vec2(6.0, 6.0), vec2(4.0, r.height() - 12.0)),
            2.0,
            colour,
        );
        if inspected {
            painter.rect_stroke(
                r.shrink(1.0),
                0.0,
                Stroke::new(1.0, tc.accent),
                StrokeKind::Inside,
            );
        }
        let (row, buttons) = header_rows(r, head);
        let indent = if lane.in_folder { 12.0 } else { 0.0 };
        // What kind of track it is, as an icon.
        let icon = match lane.kind {
            daw_project::TrackKind::Audio => Icon::Wave,
            daw_project::TrackKind::Instrument => Icon::Keys,
            daw_project::TrackKind::Bus => Icon::Speaker,
            daw_project::TrackKind::Folder => Icon::Folder,
        };
        let icon_rect = Rect::from_center_size(
            pos2(r.left() + 22.0 + indent, row.center().y),
            vec2(13.0, 13.0),
        );
        crate::icons::paint(painter, icon_rect, icon, tc.text_dim);
        // The name, cut off before the buttons (or the meter).
        let name_left = icon_rect.right() + 6.0;
        let name_right = match buttons {
            Some(b) if (b.center().y - row.center().y).abs() < 1.0 => b.left() - 4.0,
            _ => r.right() - HEAD_METER - 8.0,
        };
        let name_clip = Rect::from_x_y_ranges(name_left..=name_right.max(name_left), r.y_range());
        painter
            .with_clip_rect(name_clip.intersect(painter.clip_rect()))
            .text(
                pos2(name_left, row.center().y),
                Align2::LEFT_CENTER,
                lane.name,
                look.font(look.body),
                if head.off { tc.text_dim } else { tc.text },
            );
        // Under the name, after the buttons: whether it is off or
        // solo-safe, its edit group, faint while group editing is off,
        // and the version playing if it has others.
        let mut notes = Vec::new();
        if head.off {
            notes.push(("Off".to_string(), tc.warn));
        }
        if head.solo_safe {
            notes.push(("Solo-safe".to_string(), tc.text_dim));
        }
        if let Some(g) = groups::group_of(&self.groups, i) {
            notes.push((
                g.name.clone(),
                if self.group_edit { tc.text } else { tc.line },
            ));
        }
        if let Some((names, at)) = view.versions.get(i)
            && let Some(v) = names.get(*at)
        {
            notes.push((format!("{v} ({}/{})", at + 1, names.len()), tc.text_dim));
        }
        if !notes.is_empty() {
            let left = match buttons {
                Some(b) if b.top() > row.bottom() - 1.0 => b.right() + 6.0,
                _ => name_left,
            };
            let y = r.bottom() - 6.0;
            let clip = Rect::from_x_y_ranges(left..=r.right() - HEAD_METER - 8.0, r.y_range());
            let p = painter.with_clip_rect(clip.intersect(painter.clip_rect()));
            let mut x = left;
            for (text, c) in notes {
                let at = p.text(
                    pos2(x, y),
                    Align2::LEFT_BOTTOM,
                    text,
                    look.font(look.small),
                    c,
                );
                x = at.right() + 6.0;
            }
        }
        // The level meter at the right edge: left and right.
        let m = Rect::from_x_y_ranges(
            r.right() - HEAD_METER - 4.0..=r.right() - 4.0,
            r.top() + 5.0..=r.bottom() - 5.0,
        );
        painter.rect_filled(m, 1.0, tc.meter_bg);
        for (k, peak) in [head.meter.0, head.meter.1].into_iter().enumerate() {
            let db = if peak > 0.0 {
                20.0 * peak.log10()
            } else {
                -120.0
            };
            let level = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
            if level <= 0.0 {
                continue;
            }
            let w = m.width() / 2.0;
            let bar = Rect::from_x_y_ranges(
                m.left() + k as f32 * w..=m.left() + (k as f32 + 1.0) * w - 0.5,
                m.bottom() - level * m.height()..=m.bottom(),
            );
            let c = if db > 0.0 {
                tc.meter_high
            } else if db > -12.0 {
                tc.meter_mid
            } else {
                tc.meter_low
            };
            painter.rect_filled(bar, 0.0, c);
        }
    }

    /// Each header's mute, solo and arm buttons, lit when on, drawn over
    /// the headers so they take the clicks. A header half scrolled out of
    /// view shows none.
    fn header_buttons(
        &mut self,
        ui: &mut egui::Ui,
        song: &Song,
        heads: &[(usize, Rect, Head)],
        clip: Rect,
        out: &mut Outcome,
    ) {
        let c = crate::theme::look(ui.ctx()).c().clone();
        for &(i, r, head) in heads {
            let (_, Some(b)) = header_rows(r, head) else {
                continue;
            };
            if !clip.contains_rect(b) {
                continue;
            }
            let name = song.lanes.get(i).map_or("", |l| l.name);
            let mut at = b.left();
            let mut button = |ui: &mut egui::Ui,
                              on: bool,
                              text: &str,
                              lit: Color32,
                              spoken: String,
                              tip: &str| {
                let br = Rect::from_min_size(pos2(at, b.top()), HEAD_BUTTON);
                at += HEAD_BUTTON.x + HEAD_GAP;
                crate::mixer::lit(ui, lit, |ui| {
                    ui.put(
                        br,
                        egui::Button::selectable(on, RichText::new(text).small().strong()),
                    )
                })
                .accessible_name(spoken)
                .on_hover_text(tip)
                .clicked()
            };
            // ai: set_parameter
            if button(
                ui,
                head.mute,
                "M",
                c.warn,
                format!("Mute the {name} track"),
                "Mute",
            ) {
                out.head = Some((i, HeadAsk::Mute));
            }
            // ai: set_parameter
            if button(
                ui,
                head.solo,
                "S",
                c.good,
                format!("Solo the {name} track"),
                "Solo",
            ) {
                out.head = Some((i, HeadAsk::Solo));
            }
            if let Some(arm) = head.arm {
                // ai: set_track_input
                if button(
                    ui,
                    arm,
                    "R",
                    c.record,
                    format!("Arm the {name} track"),
                    "Arm for recording (R records)",
                ) {
                    out.head = Some((i, HeadAsk::Arm));
                }
            }
        }
    }

    /// Tempo and time-signature changes along the ruler's middle row:
    /// each as a label at its beat, with a slope up to a tempo glided into.
    fn draw_time_changes(
        &self,
        painter: &egui::Painter,
        view: &View,
        ruler: Rect,
        x_of: &dyn Fn(i64) -> f32,
        look: &Look,
    ) {
        let map = view.map;
        let c = look.c();
        let (tempo_c, meter_c) = (c.tempo, c.signature);
        let (top, bottom) = (ruler.top() + 14.0, ruler.top() + 26.0);
        let painter = painter.with_clip_rect(ruler);
        let label = |x: f32, text: String, colour: Color32| {
            let g = painter.layout_no_wrap(text, look.font(10.0), c.bg);
            let r = Rect::from_min_size(pos2(x + 1.0, top), g.size() + vec2(6.0, 0.0));
            painter.rect_filled(r, look.radius.min(3.0), colour);
            painter.galley(r.min + vec2(3.0, 0.0), g, c.bg);
            r.width()
        };
        let bpm = |t: daw_engine::time::Tempo| {
            let v = t.as_f64();
            if v.fract() == 0.0 {
                format!("{v:.0}")
            } else {
                format!("{v:.1}")
            }
        };
        let tempos: Vec<_> = map.tempo_changes().collect();
        let meters: Vec<_> = map.signature_changes().collect();
        if tempos.is_empty() && meters.is_empty() {
            return;
        }
        let start = map.signature_at(Ticks(0));
        let x0 = x_of(0);
        let w = label(x0, format!("{} BPM", bpm(map.tempo_at(Ticks(0)))), tempo_c);
        label(
            x0 + w + 2.0,
            format!("{}/{}", start.beats, start.unit),
            meter_c,
        );
        let mut prev = x0;
        for (at, t, glided) in tempos {
            let x = x_of(map.frames_at(at).0);
            if glided {
                painter.line_segment(
                    [pos2(prev, bottom), pos2(x, top)],
                    Stroke::new(1.5 * look.line, tempo_c),
                );
            }
            painter.line_segment(
                [pos2(x, top), pos2(x, bottom)],
                Stroke::new(look.line, tempo_c),
            );
            label(x, format!("{} BPM", bpm(t)), tempo_c);
            prev = x;
        }
        for (at, _, sig) in meters {
            let x = x_of(map.frames_at(at).0);
            // To the left of a tempo change on the same bar line.
            let g = format!("{}/{}", sig.beats, sig.unit);
            painter.line_segment(
                [pos2(x, top), pos2(x, bottom)],
                Stroke::new(look.line, meter_c),
            );
            let shift = if map.tempo_changes().any(|(t, _, _)| t == at) {
                -(g.len() as f32 * 6.0 + 10.0)
            } else {
                0.0
            };
            label(x + shift, g, meter_c);
        }
    }

    /// The timecode and hit point strip: click to move the playhead
    /// (to the start of a frame when timecode is shown), drag a hit
    /// point's diamond to move it, right-click to add one.
    #[allow(clippy::too_many_arguments)]
    fn strip(
        &mut self,
        ui: &egui::Ui,
        r: Rect,
        resp: &egui::Response,
        view: &View,
        x_of: &dyn Fn(i64) -> f32,
        frame_of: &dyn Fn(f32) -> i64,
        out: &mut Outcome,
    ) {
        use crate::hitpoints::{HitAsk, draw_strip, hit_at};
        let look = crate::theme::look(ui.ctx());
        let tc = look.c();
        let painter = ui.painter_at(r);
        let left = r.left() + HEADER;
        let snap = |f: i64| match &view.timecode {
            Some(c) => c.snap(f, view.rate),
            None => f,
        };
        painter.rect_filled(
            Rect::from_min_max(r.min, pos2(left, r.max.y)),
            0.0,
            tc.panel,
        );
        painter.text(
            pos2(r.left() + 6.0, r.center().y),
            Align2::LEFT_CENTER,
            if view.timecode.is_some() {
                "Timecode"
            } else {
                "Hit points"
            },
            look.font(look.small),
            tc.text_dim,
        );
        let lanes = Rect::from_min_max(pos2(left, r.top()), r.max);
        draw_strip(
            &painter,
            lanes,
            view.timecode.as_ref(),
            view.rate,
            view.hits,
            x_of,
            frame_of,
            &look,
        );
        let at_pointer = resp.interact_pointer_pos();
        if resp.drag_started()
            && let Some(p) = at_pointer
        {
            self.hit_drag = hit_at(view.hits, x_of, p).map(|i| view.hits[i].id.clone());
        }
        if resp.dragged()
            && let (Some(id), Some(p)) = (&self.hit_drag, at_pointer)
        {
            out.hit = Some(HitAsk::Move(id.clone(), frame_of(p.x).max(0)));
        }
        if resp.drag_stopped() {
            self.hit_drag = None;
        }
        if resp.clicked()
            && let Some(p) = at_pointer
            && p.x >= left
        {
            out.seek = Some(match hit_at(view.hits, x_of, p) {
                Some(i) => view.hits[i].at,
                None => snap(frame_of(p.x).max(0)),
            });
        }
        if resp.secondary_clicked()
            && let Some(p) = at_pointer
        {
            self.strip_at = frame_of(p.x).max(0);
        }
        resp.context_menu(|ui| {
            // ai: hit-point
            if ui.button("Add hit point here").clicked() {
                out.hit = Some(HitAsk::Add(self.strip_at));
                ui.close();
            }
            // ai: hit-points
            if ui.button("Timecode and hit points…").clicked() {
                out.hit = Some(HitAsk::Open);
                ui.close();
            }
        });
    }

    /// The bar the playhead is in.
    fn bar_at(map: &TempoMap, frames: i64) -> i64 {
        map.bar_beat(map.ticks_at(Frames(frames))).bar
    }

    /// The first bar after the song's end.
    pub(crate) fn end_bar(map: &TempoMap, song: &Song) -> i64 {
        Self::bar_at(map, song.end()) + 1
    }

    /// A clip or marker on the timeline has the keyboard, so letter
    /// shortcuts still apply (unlike in a text field).
    pub fn has_keyboard(&self, ctx: &egui::Context) -> bool {
        self.focus
            .as_ref()
            .is_some_and(|(_, id)| ctx.memory(|m| m.has_focus(*id)))
    }

    /// The focused clip, if there is one.
    fn focused_clip(&self) -> Option<u64> {
        match &self.focus {
            Some((Focus::Clip(id), _)) => Some(*id),
            _ => None,
        }
    }

    /// The focused marker's index, if there is one.
    fn focused_marker(&self, song: &Song) -> Option<usize> {
        match &self.focus {
            Some((Focus::Marker(id), _)) => song.markers.iter().position(|m| &m.id == id),
            _ => None,
        }
    }

    /// The groups edited together now: none while group editing is off.
    fn grouped(&self) -> &[LaneGroup] {
        if self.group_edit { &self.groups } else { &[] }
    }

    /// Makes clip `id`, and the clips edited with it, the selection.
    fn select_only(&mut self, song: &Song, id: u64) {
        self.selected = groups::with_partners(song, self.grouped(), [id]);
    }

    /// Adds clip `id`, and the clips edited with it, to the selection,
    /// or takes them out if it is in it.
    fn toggle_selected(&mut self, song: &Song, id: u64) {
        let these = groups::with_partners(song, self.grouped(), [id]);
        if self.selected.contains(&id) {
            self.selected.retain(|c| !these.contains(c));
        } else {
            self.selected.extend(these);
        }
    }

    /// Whether any selected clip is on a grouped track. Those stay on
    /// their tracks when moved, so each mic keeps its own track.
    fn selection_grouped(&self, song: &Song) -> bool {
        (self.selected.iter())
            .filter_map(|&id| song.lane_of(id))
            .any(|l| groups::group_of(self.grouped(), l).is_some())
    }

    /// Keys act on the selection; a focused clip that isn't selected
    /// becomes the selection first, as a dragged clip does.
    fn select_focused(&mut self, song: &Song) {
        if let Some(id) = self.focused_clip()
            && !self.selected.contains(&id)
        {
            self.select_only(song, id);
        }
    }

    /// Moves the selected clips by `n` steps of `grid`; none goes before
    /// the start.
    fn nudge(&mut self, song: &mut Song, map: &TempoMap, n: i64, grid: Grid) -> bool {
        self.select_focused(song);
        let ids: Vec<u64> = song
            .ordered()
            .into_iter()
            .filter(|id| self.selected.contains(id))
            .collect();
        if ids.is_empty() {
            return false;
        }
        // Each lands where its own step takes it (the tempo may
        // change between them); the move stops where the first would
        // go before the start.
        let moves: Vec<(u64, i64)> = ids
            .iter()
            .filter_map(|&id| song.find(id).map(|c| (id, step(map, c.start, n, grid))))
            .collect();
        if moves
            .iter()
            .any(|&(id, to)| n < 0 && song.find(id).is_some_and(|c| to == c.start))
        {
            return false;
        }
        for (id, to) in moves {
            if let Some(c) = song.find_mut(id) {
                c.start = to;
            }
        }
        true
    }

    /// Moves the selected clips `by` tracks down (up if negative), as
    /// far as they all fit.
    fn shift_lane(&mut self, song: &mut Song, by: isize) -> bool {
        self.select_focused(song);
        if self.selection_grouped(song) {
            return false;
        }
        let n = song.lanes.len() as isize;
        let ids: Vec<(usize, u64)> = (self.selected.iter())
            .filter_map(|&id| song.lane_of(id).map(|l| (l, id)))
            .collect();
        if ids.is_empty() {
            return false;
        }
        let lo = ids.iter().map(|&(l, _)| l).min().unwrap_or(0) as isize;
        let hi = ids.iter().map(|&(l, _)| l).max().unwrap_or(0) as isize;
        let by = by.clamp(-lo, n - 1 - hi);
        if by == 0 {
            return false;
        }
        for (l, id) in ids {
            let to = (l as isize + by) as usize;
            song.move_to_lane(id, to);
            song.detach_take(id, l);
        }
        true
    }

    /// Trims the focused clip's start or end edge by `n` steps of
    /// `grid`, and the same edge of the clips edited with it as far.
    fn trim_focused(
        &mut self,
        song: &mut Song,
        map: &TempoMap,
        n: i64,
        grid: Grid,
        end: bool,
    ) -> bool {
        let Some(id) = self.focused_clip() else {
            return false;
        };
        let Some(c) = song.find(id) else {
            return false;
        };
        let by = if end {
            step(map, c.end(), n, grid) - c.end()
        } else {
            step(map, c.start, n, grid) - c.start
        };
        let mut ids = vec![id];
        ids.extend(groups::partners(song, self.grouped(), id));
        let mut changed = false;
        for id in ids {
            changed |= self.trim_by(song, id, by, end);
        }
        changed
    }

    /// Moves clip `id`'s start or end edge `by` frames, rippling the
    /// clips after a moved end when ripple is on.
    fn trim_by(&self, song: &mut Song, id: u64, by: i64, end: bool) -> bool {
        let Some(lane) = song.lane_of(id) else {
            return false;
        };
        let clips = &mut *song.lanes[lane].clips;
        let Some(i) = clips.iter().position(|c| c.id == id) else {
            return false;
        };
        let c = &mut clips[i];
        let was = (c.start, c.offset, c.len);
        let old_end = c.end();
        if end {
            c.trim_end(old_end + by);
        } else {
            c.trim_start(c.start + by);
        }
        let changed = (c.start, c.offset, c.len) != was;
        let by = c.end() - old_end;
        if end && self.ripple && by != 0 {
            for o in clips
                .iter_mut()
                .filter(|o| o.id != id && o.start >= old_end)
            {
                o.start += by;
            }
        }
        changed
    }

    /// The grid step the keys move by: the grid as drawn now.
    fn key_grid(&self, map: &TempoMap, at: i64) -> Grid {
        self.grid.at_zoom(map, at, self.px_per_sec as f64)
    }

    /// Shows the timeline from `from` to `to` (frames), with a little
    /// room either side.
    fn zoom_to(&mut self, from: i64, to: i64, rate: u32) {
        let secs = ((to - from).max(1) as f64 / rate as f64).max(0.05);
        self.px_per_sec = ((self.width as f64 / (secs * 1.1)) as f32).clamp(1.0, 4000.0);
        let pad = 0.05 * secs;
        self.scroll = (from as f64 / rate as f64 - pad).max(0.0);
    }

    /// Zooms by `k` about the middle of the view.
    fn zoom_by(&mut self, k: f32) {
        let mid = self.scroll + self.width as f64 / 2.0 / self.px_per_sec as f64;
        self.px_per_sec = (self.px_per_sec * k).clamp(1.0, 4000.0);
        self.scroll = (mid - self.width as f64 / 2.0 / self.px_per_sec as f64).max(0.0);
    }

    /// Shows the selected clips, or, with none, the whole song.
    pub fn zoom_to_selection(&mut self, song: &Song, rate: u32) {
        let picked: Vec<&Clip> = (song.lanes.iter())
            .flat_map(|l| l.clips.iter())
            .filter(|c| self.selected.contains(&c.id))
            .collect();
        match (
            picked.iter().map(|c| c.start).min(),
            picked.iter().map(|c| c.end()).max(),
        ) {
            (Some(a), Some(b)) => self.zoom_to(a, b, rate),
            _ => self.zoom_to(0, song.end().max(10 * rate as i64), rate),
        }
    }

    /// Moves the keyboard to the clip on the next track down (up if
    /// negative) nearest in time to the focused one.
    fn focus_lane(&mut self, song: &Song, by: isize) {
        let Some(id) = self.focused_clip() else {
            return;
        };
        let (Some(lane), Some(c)) = (song.lane_of(id), song.find(id)) else {
            return;
        };
        let mut l = lane as isize + by;
        while l >= 0 && l < song.lanes.len() as isize {
            if let Some(next) = song.nearest_on(l as usize, c.start) {
                self.focus_next = Some(Focus::Clip(next));
                return;
            }
            l += by;
        }
    }

    /// The item after (or before) the focused one in Tab order, to
    /// take the keyboard when it is deleted.
    fn neighbour(&self, song: &Song) -> Option<Focus> {
        let id = self.focused_clip()?;
        let order = song.ordered();
        let i = order.iter().position(|&c| c == id)?;
        let next = (i + 1..order.len())
            .chain((0..i).rev())
            .find(|&j| !self.selected.contains(&order[j]))?;
        Some(Focus::Clip(order[next]))
    }

    /// The commands run on the timeline (by key, menu or search), and
    /// its own keys when no text field has the keyboard. With a clip or
    /// marker focused, the commands and arrow keys act on it.
    /// Draws a track's note clips: a block each, with its notes as bars
    /// placed by pitch, and its name.
    #[allow(clippy::too_many_arguments)]
    fn draw_note_clips(
        &self,
        lanes: &egui::Painter,
        look: &Look,
        links: &crate::linked::Links,
        lane: &Lane,
        top: f32,
        lane_h: f32,
        left: f32,
        right: f32,
        colour: Color32,
        view: &View,
        x_of: &dyn Fn(i64) -> f32,
    ) {
        let tc = look.c();
        let order = lane
            .notes
            .iter()
            .filter(|c| c.muted)
            .chain(lane.notes.iter().filter(|c| !c.muted));
        for c in order {
            let x0 = x_of(view.map.frames_at(Ticks(c.start)).0);
            let x1 = x_of(view.map.frames_at(Ticks(c.end())).0);
            if x1 < left || x0 > right {
                continue;
            }
            let full = Rect::from_x_y_ranges(x0..=x1, top + 2.0..=top + lane_h - 2.0);
            let r = Rect::from_x_y_ranges(x0.max(left)..=x1.min(right), full.y_range());
            let corner = look.clip_radius.min(r.height() / 2.0).min(r.width() / 2.0);
            let selected = self.note_sel.as_deref() == Some(c.id.as_str());
            if c.muted {
                lanes.rect_stroke(r, corner, Stroke::new(1.0, colour), StrokeKind::Inside);
            } else {
                lanes.rect_filled(r, corner, tc.lane);
                lanes.rect_filled(r, corner, tc.clip_fill.of(colour));
            }
            // A theme that puts the name on a paper label draws the notes
            // in the label's colour, so they show on the clip's own fill.
            let ink = if c.muted {
                tc.text_dim
            } else {
                tc.clip_label.unwrap_or_else(|| tc.clip_text.of(colour))
            };
            let clipped = lanes.with_clip_rect(r.intersect(lanes.clip_rect()));
            // The notes, placed by pitch over the range the clip uses.
            let placed: Vec<daw_project::Note> = c.placed().collect();
            if !placed.is_empty() {
                let lo = placed.iter().map(|n| n.pitch).min().unwrap_or(60) as f32;
                let hi = placed.iter().map(|n| n.pitch).max().unwrap_or(60) as f32;
                let span = (hi - lo).max(12.0);
                let body = Rect::from_x_y_ranges(x0..=x1, full.top() + 14.0..=full.bottom() - 3.0);
                // The rows stay within the body however wide the range:
                // a row thinner than a pixel still draws a pixel, so the
                // highest notes can overlap, but never climb into the name.
                let row = body.height() / (span + 1.0);
                for n in &placed {
                    let nx0 = x_of(view.map.frames_at(Ticks(n.start)).0);
                    let nx1 = x_of(view.map.frames_at(Ticks(n.end())).0).max(nx0 + 1.0);
                    let y = body.bottom() - (n.pitch as f32 - lo + 1.0) * row;
                    clipped.rect_filled(
                        Rect::from_x_y_ranges(nx0..=nx1, y..=(y + row.clamp(1.0, 3.0))),
                        0.0,
                        ink,
                    );
                }
            }
            // A looping clip: a notch along the bottom edge where each
            // time round begins, clear of the name.
            for (_, _, shift) in c.spans().into_iter().skip(1) {
                let t = c.start - c.offset + shift;
                let x = x_of(view.map.frames_at(Ticks(t)).0);
                clipped.line_segment(
                    [pos2(x, full.bottom() - 8.0), pos2(x, full.bottom())],
                    Stroke::new(1.5, ink),
                );
            }
            let mut at = r.left_top() + vec2(4.0, 2.0);
            // A pattern clip: a little grid of steps before its name.
            if c.pattern.is_some() {
                for (i, j) in [(0.0, 0.0), (5.0, 0.0), (0.0, 5.0), (5.0, 5.0)] {
                    let p = at + vec2(i, 3.0 + j);
                    clipped.rect_filled(Rect::from_min_size(p, vec2(4.0, 4.0)), 1.0, ink);
                }
                at.x += 13.0;
            }
            let font = look.font(look.small);
            let mut name = if c.muted {
                format!("{} (muted)", c.name)
            } else {
                c.name.clone()
            };
            if c.looped().is_some() {
                name.push_str(" (loops)");
            }
            if c.muted {
                clipped.text(at, Align2::LEFT_TOP, name, font, ink);
            } else {
                let g = clipped.layout_no_wrap(name, font, tc.clip_text.of(colour));
                if let Some(label) = tc.clip_label {
                    let back = Rect::from_min_size(at - vec2(3.0, 1.0), g.size() + vec2(6.0, 2.0));
                    clipped.rect_filled(back, 3.0, label);
                }
                clipped.galley(at, g, tc.clip_text.of(colour));
            }
            if selected {
                lanes.rect_stroke(r, corner, Stroke::new(2.0, tc.text), StrokeKind::Inside);
            }
            let key = crate::linked::note_key(c);
            links.draw(lanes, key.as_deref(), r, corner, selected);
        }
    }

    fn keys(&mut self, ctx: &egui::Context, song: &mut Song, view: &View, out: &mut Outcome) {
        let focused = self.has_keyboard(ctx);
        for cmd in view.commands {
            match cmd {
                Cmd::Delete => {
                    if let Some(i) = self.focused_marker(song).filter(|_| focused) {
                        song.markers.remove(i);
                    } else if self.selected.is_empty()
                        && let Some(key) = self.note_sel.take()
                    {
                        for l in &mut song.lanes {
                            let before = l.notes.len();
                            l.notes.retain(|c| c.id != key);
                            out.changed |= l.notes.len() != before;
                        }
                    } else {
                        if focused {
                            self.select_focused(song);
                            self.focus_next = self.neighbour(song);
                        }
                        out.changed |= self.delete_selected(song);
                    }
                }
                Cmd::Split => {
                    if focused {
                        self.select_focused(song);
                    }
                    out.changed |= self.split_at(song, view.playhead);
                }
                Cmd::Marker => self.add_marker(song, Self::bar_at(view.map, view.playhead)),
                Cmd::ClearSelection => self.selected.clear(),
                Cmd::Crossfade => {
                    if focused {
                        self.select_focused(song);
                    }
                    out.changed |= self.crossfade_selected(song, view.rate);
                }
                Cmd::ClipFades => {
                    if focused {
                        self.select_focused(song);
                    }
                    self.fades_open = true;
                }
                Cmd::ZoomIn => self.zoom_by(1.5),
                Cmd::ZoomOut => self.zoom_by(1.0 / 1.5),
                Cmd::ZoomSelection => self.zoom_to_selection(song, view.rate),
                Cmd::Snap => self.snap = !self.snap,
                Cmd::Ripple => self.ripple = !self.ripple,
                Cmd::VolumeLines => self.show_volume = !self.show_volume,
                Cmd::GroupEdit => self.group_edit = !self.group_edit,
                _ => {}
            }
        }
        if !focused {
            return;
        }

        let end_bar = Self::end_bar(view.map, song);
        let key = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        if let Some(i) = self.focused_marker(song) {
            // Enter goes to the section; with Ctrl it loops it too.
            if key(Modifiers::COMMAND, Key::Enter) {
                out.loop_bars = Some(section(song.markers, i, end_bar));
                out.seek = Some(
                    view.map
                        .frames_at(view.map.bar_start(song.markers[i].bar))
                        .0,
                );
            } else if key(Modifiers::NONE, Key::Enter) {
                out.seek = Some(
                    view.map
                        .frames_at(view.map.bar_start(song.markers[i].bar))
                        .0,
                );
            }
            if key(Modifiers::NONE, Key::F2) {
                self.rename = Some((i, song.markers[i].name.clone()));
                self.rename_focused = false;
            }
            // Arrows move the marker a bar, never onto another marker.
            for (k, by) in [(Key::ArrowLeft, -1), (Key::ArrowRight, 1)] {
                if key(Modifiers::NONE, k) {
                    let bar = song.markers[i].bar + by;
                    if bar >= 1 && !song.markers.iter().any(|m| m.bar == bar) {
                        song.markers[i].bar = bar;
                    }
                }
            }
            return;
        }
        let Some(id) = self.focused_clip() else {
            return;
        };
        if key(Modifiers::COMMAND, Key::Enter) {
            self.toggle_selected(song, id);
        } else if key(Modifiers::NONE, Key::Enter) {
            self.select_only(song, id);
            if let Some(c) = song.find(id) {
                out.seek = Some(c.start);
            }
        }
        let shift = Modifiers::SHIFT;
        let at = song.find(id).map_or(view.playhead, |c| c.start);
        let fine = self.key_grid(view.map, at);
        let bar = Grid::Bar;
        for (k, n) in [(Key::ArrowLeft, -1), (Key::ArrowRight, 1)] {
            // Plain arrows move the selection a grid step, Shift a bar;
            // Ctrl trims the start edge and Alt the end edge instead.
            // (The fuller combinations first: egui lets a plain key
            // match with Shift or Alt held.)
            if key(Modifiers::COMMAND | shift, k) {
                out.changed |= self.trim_focused(song, view.map, n, bar, false);
            } else if key(Modifiers::ALT | shift, k) {
                out.changed |= self.trim_focused(song, view.map, n, bar, true);
            } else if key(Modifiers::COMMAND, k) {
                out.changed |= self.trim_focused(song, view.map, n, fine, false);
            } else if key(Modifiers::ALT, k) {
                out.changed |= self.trim_focused(song, view.map, n, fine, true);
            } else if key(shift, k) {
                out.changed |= self.nudge(song, view.map, n, bar);
            } else if key(Modifiers::NONE, k) {
                out.changed |= self.nudge(song, view.map, n, fine);
            }
        }
        for (k, by) in [(Key::ArrowUp, -1), (Key::ArrowDown, 1)] {
            // Up and down move the keyboard to the next track's clips;
            // with Ctrl they move the clips themselves.
            if key(Modifiers::COMMAND, k) {
                out.changed |= self.shift_lane(song, by);
            } else if key(Modifiers::NONE, k) {
                self.focus_lane(song, by);
            }
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, song: &mut Song, view: &View, out: &mut Outcome) {
        ui.horizontal_wrapped(|ui| {
            // ai: view
            ui.checkbox(&mut self.snap, "Snap")
                .on_hover_text("Snap to the grid while dragging. Hold Alt to place freely");
            let shown = self.grid.at_zoom(view.map, view.playhead, self.px_per_sec as f64);
            let text = match self.grid {
                Grid::Auto => format!("Auto: {}", shown.label()),
                g => g.label().to_string(),
            };
            egui::ComboBox::from_id_salt("grid")
                .width(110.0)
                .selected_text(text)
                .show_ui(ui, |ui| {
                    for g in Grid::ALL {
                        // ai: view
                        ui.selectable_value(&mut self.grid, g, g.label());
                    }
                })
                .response
                .accessible_name("Grid")
                .on_hover_text(
                    "The grid's step, drawn on the timeline. Auto follows the zoom. \
                     The arrow keys move clips by it",
                );
            if !self.simple {
                // ai: person
                ui.checkbox(&mut self.magnet, "To clips")
                    .on_hover_text("Also snap to other clips' edges, markers and the playhead");
                // ai: ripple
                ui.checkbox(&mut self.ripple, "Ripple").on_hover_text(
                    "Deleting a clip, or trimming its end, moves the later clips on its track to close the gap",
                );
                // ai: volume-lines
                ui.checkbox(&mut self.show_volume, "Volume").on_hover_text(
                    "Show each clip's volume line: drag it up or down, double-click to add a point",
                );
            }
            if !self.groups.is_empty() && !self.simple {
                // ai: group-edit
                ui.checkbox(&mut self.group_edit, "Groups").on_hover_text(
                    "Edit the clips on grouped tracks together. Untick to edit one track alone",
                );
            }
            ui.separator();
            if ui
                // ai: zoom-out
                .button("−")
                .accessible_name("Zoom out")
                .on_hover_text("Zoom out (Ctrl + wheel)")
                .clicked()
            {
                self.zoom_by(1.0 / 1.5);
            }
            if ui
                // ai: zoom-in
                .button("+")
                .accessible_name("Zoom in")
                .on_hover_text("Zoom in (Ctrl + wheel)")
                .clicked()
            {
                self.zoom_by(1.5);
            }
            if ui
                // ai: view
                .button("Fit")
                .on_hover_text("Show the whole song")
                .clicked()
            {
                let secs = (song.end() as f64 / view.rate as f64).max(10.0);
                let w = (ui.available_width() + ui.min_rect().width() - HEADER).max(200.0);
                self.px_per_sec = (w as f64 / (secs * 1.05)) as f32;
                self.scroll = 0.0;
            }
            if ui
                // ai: zoom-selection
                .add_enabled(!self.selected.is_empty(), egui::Button::new("Zoom to clips"))
                .on_hover_text("Show the selected clips across the timeline (Z)")
                .clicked()
            {
                self.zoom_to_selection(song, view.rate);
            }
            ui.separator();
            if ui
                // ai: split
                .add(IconButton::labelled(Icon::Split, "Split"))
                .on_hover_text("Split the selected clips, or all of them, at the playhead (S)")
                .clicked()
            {
                out.changed |= self.split_at(song, view.playhead);
            }
            if ui
                .add_enabled(
                    !self.selected.is_empty(),
                    // ai: delete
                    IconButton::labelled(Icon::Delete, "Delete").spoken("Delete the selected clips"),
                )
                .on_hover_text("Delete the selected clips (Delete)")
                .clicked()
            {
                out.changed |= self.delete_selected(song);
            }
            if ui
                // ai: crossfade
                .button("Crossfade")
                .on_hover_text(
                    "Fade clips into each other where they meet: the selected ones, or all (X)",
                )
                .clicked()
            {
                out.changed |= self.crossfade_selected(song, view.rate);
            }
            if ui
                // ai: clip-fades
                .add_enabled(!self.selected.is_empty(), egui::Button::new("Fades…"))
                .on_hover_text("Fades and volume of the selected clips (F)")
                .clicked()
            {
                self.fades_open = true;
            }
            if ui
                // ai: marker
                .add(IconButton::labelled(Icon::Marker, "Marker"))
                .on_hover_text("Mark a section at the playhead's bar (M)")
                .clicked()
            {
                self.add_marker(song, Self::bar_at(view.map, view.playhead));
            }
            ui.separator();
            if ui
                // ai: undo
                .add_enabled(view.can_undo, IconButton::new(Icon::Undo, "Undo").tip("Undo (Ctrl+Z)"))
                .clicked()
            {
                out.undo = true;
            }
            if ui
                // ai: redo
                .add_enabled(view.can_redo, IconButton::new(Icon::Redo, "Redo").tip("Redo (Ctrl+Shift+Z)"))
                .clicked()
            {
                out.redo = true;
            }
        });
    }

    /// Draws the timeline and handles editing. Fills the space given.
    pub fn show(&mut self, ui: &mut egui::Ui, song: &mut Song, view: &View) -> Outcome {
        let mut out = Outcome::default();
        if self.waves.poll() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.toolbar(ui, song, view, &mut out);
        // Timecode and hit points, on their own line above the ruler.
        let strip = (view.timecode.is_some() || !view.hits.is_empty()).then(|| {
            ui.allocate_exact_size(
                vec2(ui.available_width(), crate::hitpoints::STRIP),
                Sense::click_and_drag(),
            )
        });

        let size = vec2(
            ui.available_width(),
            ui.available_height().max(RULER + self.band + 40.0),
        );
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
        // The tracks start under the ruler and the band.
        let tracks_top = RULER + self.band;
        if self.band > 0.0 {
            out.band = Some(Rect::from_x_y_ranges(
                rect.x_range(),
                rect.top() + RULER..=rect.top() + tracks_top,
            ));
        }
        let lanes_n = song.lanes.len().max(1);
        // A lane with a notation band is its row and the band under it.
        let band_h = |i: usize| {
            (view.bands.get(i))
                .and_then(Option::as_ref)
                .map_or(0.0, crate::notation::Band::height)
        };
        let bands: f32 = (0..song.lanes.len()).map(band_h).sum();
        let lane_h = ((rect.height() - tracks_top - bands) / lanes_n as f32).clamp(36.0, 80.0);
        // Where each lane starts down the lanes, and where the last ends.
        let mut offs = Vec::with_capacity(lanes_n + 1);
        let mut down = 0.0;
        for i in 0..lanes_n {
            offs.push(down);
            down += lane_h + band_h(i);
        }
        offs.push(down);
        let offs = &offs;
        // Lanes that don't fit scroll up and down.
        let overflow = (down - (rect.height() - tracks_top)).max(0.0);
        let left = rect.left() + HEADER;
        self.width = (rect.right() - left).max(50.0);
        let rate = view.rate as f64;
        let pps = self.px_per_sec as f64;

        // Wheel scrolls, Ctrl + wheel zooms about the pointer.
        if let Some(p) = resp.hover_pos() {
            let (zoom, scroll) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta));
            if zoom != 1.0 {
                let at = self.scroll + (p.x - left) as f64 / pps;
                self.px_per_sec = (self.px_per_sec * zoom).clamp(1.0, 4000.0);
                self.scroll = (at - (p.x - left) as f64 / self.px_per_sec as f64).max(0.0);
            } else if scroll != egui::Vec2::ZERO {
                // The wheel scrolls the lanes if they overflow, or else
                // along; Shift + wheel (or a sideways wheel) along.
                let mut along = scroll.x;
                if overflow > 0.0 {
                    self.vscroll -= scroll.y;
                } else {
                    along += scroll.y;
                }
                self.scroll = (self.scroll - along as f64 / pps).max(0.0);
            }
        }
        self.vscroll = self.vscroll.clamp(0.0, overflow);
        let vscroll = self.vscroll;
        let pps = self.px_per_sec as f64;
        let scroll = self.scroll;
        let x_of = move |f: i64| left + ((f as f64 / rate - scroll) * pps) as f32;
        let frame_of = move |x: f32| ((scroll + (x - left) as f64 / pps) * rate).round() as i64;
        let lane_top = |i: usize| rect.top() + tracks_top + offs[i.min(lanes_n)] - vscroll;
        // The track at a height, unheld, then held to the first and last.
        let lane_under = move |y: f32| {
            let down = y - rect.top() - tracks_top + vscroll;
            offs[1..].partition_point(|&o| o <= down)
        };
        let lane_at = move |y: f32| lane_under(y).min(lanes_n - 1);

        if let Some((sr, sresp)) = &strip {
            self.strip(ui, *sr, sresp, view, &x_of, &frame_of, &mut out);
        }

        // Keep the playhead in view while playing: a page at a time, or
        // with the lanes moving under it a third of the way along.
        if view.playing && self.drag.is_none() && self.continuous {
            let to = (view.playhead as f64 / rate - 0.33 * (rect.width() - HEADER) as f64 / pps)
                .max(0.0);
            if (to - self.scroll).abs() > 1e-6 {
                self.scroll = to;
                ui.ctx().request_repaint();
            }
        } else if view.playing && self.drag.is_none() {
            let x = x_of(view.playhead);
            if x > rect.right() - 20.0 || x < left {
                self.scroll =
                    view.playhead as f64 / rate - 0.1 * (rect.width() - HEADER) as f64 / pps;
                self.scroll = self.scroll.max(0.0);
                ui.ctx().request_repaint();
            }
        }

        let show_volume = self.show_volume;
        let hit = |pos: Pos2, song: &Song| -> Option<Hit> {
            if !rect.contains(pos) || pos.x < left {
                return None;
            }
            for (i, p) in view.pins.iter().enumerate() {
                let top = match p.lane {
                    Some(l) => lane_top(l),
                    None => rect.top() + RULER - PIN.y,
                };
                if pin_rect(x_of(p.start), top).expand(2.0).contains(pos) {
                    return Some(Hit::Pin(i));
                }
            }
            if pos.y < rect.top() + RULER {
                for (i, m) in song.markers.iter().enumerate() {
                    let x = x_of(view.map.frames_at(view.map.bar_start(m.bar)).0);
                    if (pos.x - x).abs() < 5.0
                        || (pos.x > x && pos.x < x + 70.0 && pos.y < rect.top() + 14.0)
                    {
                        return Some(Hit::Marker(i));
                    }
                }
                return Some(Hit::Ruler);
            }
            if pos.y < rect.top() + tracks_top {
                return None;
            }
            let lane = lane_under(pos.y);
            if lane < song.lanes.len() && pos.y > lane_top(lane) + lane_h {
                return Some(Hit::Band(lane));
            }
            let notes = &song.lanes.get(lane)?.notes;
            for (index, c) in notes.iter().enumerate().rev() {
                let x0 = x_of(view.map.frames_at(Ticks(c.start)).0);
                let x1 = x_of(view.map.frames_at(Ticks(c.end())).0);
                if pos.x >= x0 && pos.x <= x1 {
                    let edge = x1 - pos.x < 6.0 && x1 - x0 > 16.0;
                    return Some(Hit::NoteClip { lane, index, edge });
                }
            }
            let clips = &song.lanes.get(lane)?.clips;
            // Playing clips over muted ones; later ones on top.
            let order = clips
                .iter()
                .rev()
                .filter(|c| !c.muted)
                .chain(clips.iter().rev().filter(|c| c.muted));
            let top = lane_top(lane);
            for c in order {
                let (x0, x1) = (x_of(c.start), x_of(c.end()));
                if pos.x >= x0 && pos.x <= x1 {
                    let r = Rect::from_x_y_ranges(x0..=x1, top + 2.0..=top + lane_h - 2.0);
                    let zone = clip_zone(c, view.map, pos, r, &x_of, show_volume && !c.muted);
                    return Some(Hit::Clip { id: c.id, zone });
                }
            }
            Some(Hit::Lane)
        };

        // The loop's ends, at the foot of the ruler, drag to new bars.
        let loop_end_at = |p: Pos2| -> Option<usize> {
            let (a, b) = view.loop_bars?;
            if !(rect.top() + RULER - 14.0..=rect.top() + RULER).contains(&p.y) {
                return None;
            }
            [a, b].iter().position(|&bar| {
                let x = x_of(view.map.frames_at(view.map.bar_start(bar)).0);
                x >= left && (p.x - x).abs() < 6.0
            })
        };

        // Pointer: cursors, selection, dragging.
        let modifiers = ui.input(|i| i.modifiers);
        let (grid, snapping) = (self.grid, self.snap);
        // Other clips' edges, the markers and the playhead, when snapping
        // to them; not the clips being dragged.
        let mut targets: Vec<i64> = Vec::new();
        if self.magnet {
            let held: HashSet<u64> = (self.drag.iter())
                .flat_map(|d| d.originals.iter().map(|(_, c)| c.id))
                .collect();
            for c in song.lanes.iter().flat_map(|l| l.clips.iter()) {
                if !held.contains(&c.id) {
                    targets.extend([c.start, c.end()]);
                }
            }
            targets.extend(
                (song.markers.iter()).map(|m| view.map.frames_at(view.map.bar_start(m.bar)).0),
            );
            targets.push(view.playhead);
        }
        let near = (MAGNET as f64 / pps * rate) as i64;
        let snap_to = |f: i64| {
            if modifiers.alt {
                return f;
            }
            if let Some(&t) = (targets.iter())
                .filter(|&&t| (t - f).abs() <= near)
                .min_by_key(|&&t| (t - f).abs())
            {
                return t;
            }
            if !snapping {
                return f;
            }
            snap(view.map, f, grid.at_zoom(view.map, f, pps))
        };
        if let Some(p) = resp.hover_pos()
            && (self.loop_drag.is_some() || loop_end_at(p).is_some())
        {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        } else if let Some(p) = resp.hover_pos()
            && self.drag.is_none()
        {
            match hit(p, song) {
                Some(Hit::Clip {
                    zone:
                        Zone::StartEdge | Zone::EndEdge | Zone::FadeIn | Zone::FadeOut | Zone::Marker(_),
                    ..
                }) => ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal),
                Some(Hit::Clip {
                    zone: Zone::Volume(None),
                    ..
                }) => ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical),
                Some(Hit::Clip {
                    zone: Zone::Volume(Some(_)),
                    ..
                }) => ui.ctx().set_cursor_icon(CursorIcon::Move),
                Some(Hit::Clip { .. }) if modifiers.command => {
                    ui.ctx().set_cursor_icon(CursorIcon::ResizeColumn)
                }
                Some(Hit::Clip { .. }) => ui.ctx().set_cursor_icon(CursorIcon::Grab),
                Some(Hit::NoteClip { edge: true, .. }) => {
                    ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal)
                }
                Some(Hit::NoteClip { .. }) => ui.ctx().set_cursor_icon(CursorIcon::Grab),
                _ => {}
            }
        }
        // Where the press began, not where the drag was noticed.
        if resp.drag_started()
            && let Some(p) = ui.input(|i| i.pointer.press_origin())
        {
            match hit(p, song) {
                _ if loop_end_at(p).is_some() => self.loop_drag = loop_end_at(p),
                Some(Hit::Clip { id, zone }) => {
                    // Ctrl held as a clip's body is dragged slips its
                    // audio; it doesn't add to the selection.
                    let slip = zone == Zone::Body && modifiers.command;
                    if !self.selected.contains(&id) {
                        if !(modifiers.shift || modifiers.command) || slip {
                            self.selected.clear();
                        }
                        self.toggle_selected(song, id);
                    }
                    // An edge or fade drags the clip's own, and the same
                    // one on the clips edited with it.
                    let edged = groups::with_partners(song, self.grouped(), [id]);
                    let held: Vec<(usize, Clip)> = song
                        .lanes
                        .iter()
                        .enumerate()
                        .flat_map(|(l, lane)| lane.clips.iter().map(move |c| (l, c)))
                        .filter(|(_, c)| {
                            if zone == Zone::Body {
                                self.selected.contains(&c.id)
                            } else if matches!(zone, Zone::Volume(_)) {
                                c.id == id
                            } else {
                                edged.contains(&c.id)
                            }
                        })
                        .map(|(l, c)| (l, c.clone()))
                        .collect();
                    let after = match zone {
                        Zone::EndEdge if self.ripple => (held.iter())
                            .flat_map(|(l, c)| {
                                (song.lanes[*l].clips.iter())
                                    .filter(|o| !edged.contains(&o.id) && o.start >= c.end())
                                    .map(|o| (o.id, o.start))
                            })
                            .collect(),
                        _ => Vec::new(),
                    };
                    let grouped = self.selection_grouped(song);
                    self.drag = Some(Drag {
                        zone,
                        anchor: id,
                        from: frame_of(p.x),
                        from_lane: lane_at(p.y),
                        from_y: p.y,
                        originals: held,
                        slip,
                        after,
                        grouped,
                        moved: false,
                    });
                    self.focus_next = Some(Focus::Clip(id));
                }
                Some(Hit::Ruler | Hit::Lane) if p.y < rect.top() + RULER => self.scrubbing = true,
                Some(Hit::NoteClip { lane, index, edge }) => {
                    if let Some(c) = song.lanes[lane].notes.get(index) {
                        self.selected.clear();
                        self.note_sel = Some(c.id.clone());
                        self.note_drag = Some(NoteDrag {
                            lane,
                            key: c.id.clone(),
                            from: frame_of(p.x),
                            start: c.start,
                            len: c.len,
                            edge,
                            moved: false,
                        });
                    }
                }
                _ => {}
            }
        }
        if let Some(d) = &mut self.note_drag
            && let Some(p) = resp.interact_pointer_pos()
            && let Some(c) = song.lanes[d.lane].notes.iter_mut().find(|c| c.id == d.key)
        {
            let map = view.map;
            let dx = frame_of(p.x) - d.from;
            if d.edge {
                let end = map.frames_at(Ticks(d.start + d.len)).0 + dx;
                let end = map.ticks_at(Frames(snap_to(end))).0;
                let len = (end - d.start).max(MIN_NOTE_CLIP);
                if len != c.len {
                    c.len = len;
                    d.moved = true;
                    out.changed = true;
                }
            } else {
                let start = map.frames_at(Ticks(d.start)).0 + dx;
                let start = map.ticks_at(Frames(snap_to(start).max(0))).0;
                if start != c.start {
                    c.start = start;
                    d.moved = true;
                    out.changed = true;
                }
            }
        }
        if self.scrubbing
            && let Some(p) = resp.interact_pointer_pos()
        {
            out.seek = Some(snap_to(frame_of(p.x.max(left))).max(0));
        }
        if let Some(end) = self.loop_drag
            && let Some(p) = resp.interact_pointer_pos()
            && let Some((a, b)) = view.loop_bars
        {
            // To the nearer bar line.
            let t = view.map.ticks_at(Frames(frame_of(p.x.max(left)).max(0)));
            let bar = view.map.bar_beat(t).bar;
            let (from, to) = (view.map.bar_start(bar).0, view.map.bar_start(bar + 1).0);
            let bar = if t.0 - from > to - t.0 { bar + 1 } else { bar };
            let new = if end == 0 {
                (bar.clamp(1, b - 1), b)
            } else {
                (a, bar.max(a + 1))
            };
            if new != (a, b) {
                out.loop_bars = Some(new);
            }
        }
        if let Some(d) = &mut self.drag
            && let Some(p) = resp.interact_pointer_pos()
        {
            let dx = frame_of(p.x) - d.from;
            if let Some((anchor_lane, anchor)) = d
                .originals
                .iter()
                .find(|(_, c)| c.id == d.anchor)
                .map(|(l, c)| (*l, c.clone()))
            {
                // The anchor's box when pressed, for the volume line.
                let top = lane_top(anchor_lane);
                let r = Rect::from_x_y_ranges(
                    x_of(anchor.start)..=x_of(anchor.end()),
                    top + 2.0..=top + lane_h - 2.0,
                );
                let mut set = |song: &mut Song, t: Clip| {
                    if let Some(c) = song.find_mut(t.id) {
                        d.moved |= (c.start, c.offset, c.len, c.fade_in, c.fade_out, c.gain_db)
                            != (t.start, t.offset, t.len, t.fade_in, t.fade_out, t.gain_db)
                            || c.envelope != t.envelope
                            || c.markers != t.markers;
                        *c = t;
                    }
                };
                match d.zone {
                    Zone::Body if d.slip => {
                        for (_, o) in &d.originals {
                            let mut t = o.clone();
                            t.slip_to(o.offset as i64 - dx);
                            set(song, t);
                        }
                    }
                    Zone::Body => {
                        // The held clip lands on the grid; the rest keep
                        // their distance from it, in time and in tracks.
                        let lowest = d.originals.iter().map(|(_, c)| c.start).min().unwrap_or(0);
                        let mut delta = (snap_to(anchor.start + dx) - anchor.start).max(-lowest);
                        // Or its end on another clip's edge.
                        let by_end = snap_to(anchor.end() + dx) - anchor.end();
                        if by_end != dx
                            && targets.contains(&(anchor.end() + by_end))
                            && !targets.contains(&(anchor.start + delta))
                        {
                            delta = by_end.max(-lowest);
                        }
                        let lo = d.originals.iter().map(|(l, _)| *l).min().unwrap_or(0) as isize;
                        let hi = d.originals.iter().map(|(l, _)| *l).max().unwrap_or(0) as isize;
                        let dl = match d.grouped {
                            true => 0,
                            false => (lane_at(p.y) as isize - d.from_lane as isize)
                                .clamp(-lo, lanes_n as isize - 1 - hi),
                        };
                        for (l, o) in &d.originals {
                            let to = (*l as isize + dl) as usize;
                            d.moved |= song.move_to_lane(o.id, to);
                            if let Some(c) = song.find_mut(o.id) {
                                d.moved |= c.start != o.start + delta;
                                c.start = o.start + delta;
                            }
                        }
                    }
                    // The held clip's edge lands on the grid; grouped
                    // clips' edges move as far.
                    Zone::StartEdge => {
                        let by = snap_to(anchor.start + dx) - anchor.start;
                        for (_, o) in &d.originals {
                            let mut t = o.clone();
                            t.trim_start(o.start + by);
                            set(song, t);
                        }
                    }
                    Zone::EndEdge => {
                        let by = snap_to(anchor.end() + dx) - anchor.end();
                        // Ripple moves what follows by as far as the held
                        // clip's end actually went.
                        let mut moved = by;
                        for (_, o) in &d.originals {
                            let mut t = o.clone();
                            t.trim_end(o.end() + by);
                            if o.id == anchor.id {
                                moved = t.end() - o.end();
                            }
                            set(song, t);
                        }
                        for &(id, start) in &d.after {
                            if let Some(c) = song.find_mut(id) {
                                c.start = start + moved;
                            }
                        }
                    }
                    Zone::FadeIn | Zone::FadeOut => {
                        let end = d.zone == Zone::FadeOut;
                        let at = snap_to(frame_of(p.x));
                        let len = if end {
                            anchor.end() - at
                        } else {
                            at - anchor.start
                        };
                        for (_, o) in &d.originals {
                            let mut t = o.clone();
                            t.set_fade(end, len.max(0) as usize);
                            set(song, t);
                        }
                    }
                    Zone::Volume(None) => {
                        let mut t = anchor.clone();
                        let db = anchor.gain_db + y_db(r, p.y) - y_db(r, d.from_y);
                        t.gain_db = db.clamp(MIN_DB, MAX_DB);
                        set(song, t);
                    }
                    Zone::Volume(Some(i)) if i < anchor.envelope.len() => {
                        let mut t = anchor.clone();
                        // Between its neighbours, within the clip.
                        let lo = (i.checked_sub(1).map(|j| anchor.envelope[j].at + 1))
                            .unwrap_or(anchor.offset);
                        let hi = (anchor.envelope.get(i + 1).map(|q| q.at - 1))
                            .unwrap_or(anchor.offset + anchor.len)
                            .max(lo);
                        let at = (anchor.envelope[i].at as i64 + dx).clamp(lo as i64, hi as i64);
                        t.envelope[i] = GainPoint {
                            at: at as usize,
                            db: (y_db(r, p.y) - anchor.gain_db).clamp(MIN_DB, MAX_DB),
                        };
                        set(song, t);
                    }
                    Zone::Volume(Some(_)) => {}
                    Zone::Marker(i) if i < anchor.markers.len() => {
                        // The marker lands on the grid; the same place in
                        // the clips edited with it moves with it.
                        let was = anchor.out_of_frame(view.map, anchor.markers[i].frame as f64);
                        let to = snap_to(was + dx);
                        for (_, o) in &d.originals {
                            let mut t = o.clone();
                            let j = if o.id == anchor.id {
                                Some(i)
                            } else if was > o.start && was < o.end() {
                                t.add_marker(t.frame_of_out(view.map, was).round() as i64)
                            } else {
                                None
                            };
                            if let Some(j) = j {
                                let at = t.at_of_out(view.map, to).round() as i64;
                                t.move_marker(j, at);
                                t.retime(view.map);
                                set(song, t);
                            }
                        }
                    }
                    Zone::Marker(_) => {}
                }
            }
        }
        if resp.drag_stopped() {
            self.scrubbing = false;
            self.loop_drag = None;
            if let Some(d) = self.note_drag.take()
                && d.moved
            {
                out.changed = true;
            }
            if let Some(d) = self.drag.take()
                && d.moved
            {
                // A take dropped on another track stops being one.
                for (l, o) in &d.originals {
                    if song.lane_of(o.id) != Some(*l) {
                        song.detach_take(o.id, *l);
                    }
                }
                out.changed = true;
            }
        }
        if resp.clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            let h = hit(p, song);
            out.off_band = !matches!(h, Some(Hit::Band(_)));
            if h.is_none() && p.x < left && p.y >= rect.top() + tracks_top {
                let lane = lane_under(p.y);
                if lane < song.lanes.len() {
                    out.head = Some((lane, HeadAsk::Select));
                }
            }
            match h {
                Some(Hit::Header(_)) => {}
                Some(Hit::Clip { id, .. }) => {
                    if modifiers.shift || modifiers.command {
                        self.toggle_selected(song, id);
                    } else {
                        self.select_only(song, id);
                    }
                    self.focus_next = Some(Focus::Clip(id));
                }
                Some(Hit::Pin(i)) => {
                    out.pin = Some(view.pins[i].id.clone());
                    out.seek = Some(view.pins[i].start);
                }
                Some(Hit::Marker(i)) => {
                    // To the start of its section.
                    let bar = view.map.bar_start(song.markers[i].bar);
                    out.seek = Some(view.map.frames_at(bar).0);
                    self.focus_next = Some(Focus::Marker(song.markers[i].id.clone()));
                }
                Some(Hit::Ruler | Hit::Lane) => {
                    self.selected.clear();
                    self.note_sel = None;
                    out.seek = Some(snap_to(frame_of(p.x)).max(0));
                }
                Some(Hit::Band(lane)) => {
                    let tick = view.map.ticks_at(Frames(frame_of(p.x).max(0))).0;
                    out.notation_band = Some((lane, tick, p.y - lane_top(lane) - lane_h));
                }
                Some(Hit::NoteClip { lane, index, .. }) => {
                    self.selected.clear();
                    self.note_sel = song.lanes[lane].notes.get(index).map(|c| c.id.clone());
                }
                None => {}
            }
        }
        if resp.double_clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            match hit(p, song) {
                Some(Hit::NoteClip { lane, index, .. }) => {
                    if let Some(c) = song.lanes[lane].notes.get(index) {
                        self.note_sel = Some(c.id.clone());
                        if c.pattern.is_some() {
                            out.pattern = Some(PatternAsk::Edit(c.id.clone()));
                        } else {
                            out.edit_notes = Some(c.id.clone());
                        }
                    }
                }
                // An empty place on an instrument track: a new clip.
                Some(Hit::Lane) if p.y >= rect.top() + RULER => {
                    let lane = lane_at(p.y);
                    if song.lanes[lane].kind.plays_notes() {
                        let n = song.lanes.iter().map(|l| l.notes.len()).sum::<usize>() + 1;
                        let c = new_note_clip(view.map, snap_to(frame_of(p.x)), n);
                        self.note_sel = Some(c.id.clone());
                        out.edit_notes = Some(c.id.clone());
                        song.lanes[lane].notes.push(c);
                        out.changed = true;
                    }
                }
                // An audio clip: its editor, in the dock.
                Some(Hit::Clip {
                    id,
                    zone: Zone::Body,
                }) => {
                    self.select_only(song, id);
                    out.edit_audio = true;
                }
                Some(Hit::Marker(i)) => {
                    self.rename = Some((i, song.markers[i].name.clone()));
                    self.rename_focused = false;
                }
                Some(Hit::Clip {
                    id,
                    zone: Zone::Volume(None),
                }) => {
                    let lane = lane_at(p.y);
                    let top = lane_top(lane);
                    if let Some(c) = song.find_mut(id) {
                        let r = Rect::from_x_y_ranges(
                            x_of(c.start)..=x_of(c.end()),
                            top + 2.0..=top + lane_h - 2.0,
                        );
                        let rel = frame_of(p.x) - c.start;
                        let db = y_db(r, p.y) - c.gain_db;
                        c.add_point(rel, db);
                        out.changed = true;
                    }
                }
                _ => {}
            }
        }
        if resp.secondary_clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            self.menu = hit(p, song).map(|h| (h, frame_of(p.x).max(0)));
            self.menu_lane = (p.y >= rect.top() + tracks_top)
                .then(|| lane_under(p.y))
                .filter(|&l| l < song.lanes.len());
            if self.menu.is_none()
                && p.x < left
                && let Some(l) = self.menu_lane
            {
                self.menu = Some((Hit::Header(l), 0));
            }
            if let Some((Hit::Clip { id, .. }, _)) = self.menu
                && !self.selected.contains(&id)
            {
                self.select_only(song, id);
            }
        }

        // Every clip and marker is an entry in the accessibility tree,
        // with a spoken name, that Tab reaches: track by track, each in
        // time order, then the markers. The arrow keys stay with the
        // focused item (egui would move the focus with them otherwise).
        let arrows = egui::EventFilter {
            horizontal_arrows: true,
            vertical_arrows: true,
            ..Default::default()
        };
        let focus_next = self.focus_next.take();
        let mut focused = None;
        let mut node = |ui: &mut egui::Ui, item: Focus, r: Rect, info: egui::WidgetInfo| {
            let id = item.id(ui);
            let resp = ui.interact(r, id, Sense::focusable_noninteractive());
            resp.widget_info(|| info.clone());
            if resp.has_focus() {
                ui.memory_mut(|m| m.set_focus_lock_filter(id, arrows));
                focused = Some((item.clone(), id));
            }
            if focus_next.as_ref() == Some(&item) {
                resp.request_focus();
            }
        };
        for (i, lane) in song.lanes.iter().enumerate() {
            let top = lane_top(i);
            let mut order: Vec<&Clip> = lane.clips.iter().collect();
            order.sort_by_key(|c| (c.start, c.id));
            let group = match groups::group_of(self.grouped(), i) {
                Some(g) => format!(" (group {})", g.name),
                None => String::new(),
            };
            for c in order {
                let r = Rect::from_x_y_ranges(
                    x_of(c.start)..=x_of(c.end()),
                    top + 2.0..=top + lane_h - 2.0,
                );
                let name = format!(
                    "{} on {}{}, {} to {}{}{}",
                    c.name(),
                    lane.name,
                    group,
                    spoken(view.map, c.start),
                    spoken(view.map, c.end()),
                    if c.muted { ", muted" } else { "" },
                    spoken_level(c, view.rate),
                );
                let selected = self.selected.contains(&c.id);
                let info = egui::WidgetInfo::selected(egui::Role::Button, true, selected, name);
                node(ui, Focus::Clip(c.id), r, info);
            }
        }
        for m in song.markers.iter() {
            let x = x_of(view.map.frames_at(view.map.bar_start(m.bar)).0);
            let r = Rect::from_min_size(pos2(x, rect.top()), vec2(70.0, 14.0));
            let name = format!("{}, marker at bar {}", m.name, m.bar);
            let info = egui::WidgetInfo::labeled(egui::Role::Button, true, name);
            node(ui, Focus::Marker(m.id.clone()), r, info);
        }
        if focused.is_some() && focused != self.focus {
            // Newly focused: bring it into view.
            let (start, lane) = match &focused {
                Some((Focus::Clip(id), _)) => {
                    let c = song.find(*id);
                    (c.map_or(0, |c| c.start), song.lane_of(*id))
                }
                Some((Focus::Marker(id), _)) => {
                    let bar = song
                        .markers
                        .iter()
                        .find(|m| &m.id == id)
                        .map_or(1, |m| m.bar);
                    (view.map.frames_at(view.map.bar_start(bar)).0, None)
                }
                None => (0, None),
            };
            let x = x_of(start);
            if x < left || x > rect.right() - 40.0 {
                self.scroll =
                    (start as f64 / rate - 0.1 * (rect.width() - HEADER) as f64 / pps).max(0.0);
            }
            if let Some(l) = lane {
                let (top, bottom) = (lane_top(l), lane_top(l) + lane_h);
                if top < rect.top() + tracks_top {
                    self.vscroll -= rect.top() + tracks_top - top;
                } else if bottom > rect.bottom() {
                    self.vscroll += bottom - rect.bottom();
                }
                self.vscroll = self.vscroll.clamp(0.0, overflow);
            }
            ui.ctx().request_repaint();
        }
        self.focus = focused;
        self.keys(&ui.ctx().clone(), song, view, &mut out);

        // Drawing.
        // Every colour and weight from here on is the theme's.
        let painter = ui.painter_at(rect);
        let look = crate::theme::look(ui.ctx());
        let tc = look.c();
        painter.rect_filled(rect, 0.0, tc.lane);
        if look.texture == Texture::Grain {
            draw_grain(&painter, rect, tc.texture);
        }
        let ruler = Rect::from_min_max(
            pos2(left, rect.top()),
            pos2(rect.right(), rect.top() + RULER),
        );
        painter.rect_filled(ruler, 0.0, tc.panel);
        painter.line_segment(
            [ruler.left_bottom(), ruler.right_bottom()],
            Stroke::new(look.line, tc.line),
        );

        // Loop range, with its ends marked so it reads without colour.
        if let Some((a, b)) = view.loop_bars {
            let fa = view.map.frames_at(view.map.bar_start(a)).0;
            let fb = view.map.frames_at(view.map.bar_start(b)).0;
            let r = Rect::from_x_y_ranges(
                x_of(fa).max(left)..=x_of(fb).max(left),
                rect.top() + RULER - 12.0..=rect.top() + RULER,
            );
            match look.loop_fill {
                LoopFill::Filled => {
                    painter.rect_filled(r, 0.0, tc.loop_range);
                }
                LoopFill::Hatched => {
                    let hatch = painter.with_clip_rect(r.intersect(painter.clip_rect()));
                    let mut x = r.left() - r.height();
                    while x < r.right() {
                        hatch.line_segment(
                            [pos2(x, r.bottom()), pos2(x + r.height(), r.top())],
                            Stroke::new(1.5, tc.loop_range.gamma_multiply(2.0)),
                        );
                        x += 5.0;
                    }
                }
            }
            for x in [x_of(fa), x_of(fb)] {
                if x >= left {
                    painter.line_segment(
                        [pos2(x, r.top()), pos2(x, r.bottom())],
                        Stroke::new(2.0, tc.accent),
                    );
                }
            }
        }

        // Bar lines, thinned out when zoomed out.
        // Bars change length with the tempo and time signature, so this
        // is measured where the view starts.
        let first = Self::bar_at(view.map, frame_of(left).max(0));
        let bar_px = {
            let b1 = view.map.frames_at(view.map.bar_start(first)).0;
            let b2 = view.map.frames_at(view.map.bar_start(first + 1)).0;
            (x_of(b2) - x_of(b1)).max(0.1)
        };
        let every = [1, 2, 4, 8, 16, 32, 64]
            .into_iter()
            .find(|&n| bar_px * n as f32 >= 40.0)
            .unwrap_or(128);
        // The grid's lines inside the bars, fainter, beats stronger than
        // their parts; each bar measured on its own, as the tempo and
        // time signature may change in it.
        let grid = self.grid.at_zoom(view.map, frame_of(left).max(0), pps);
        if every == 1 && grid != Grid::Bar {
            let (beat_c, part_c) = (tc.grid_fine, tc.grid_fine.gamma_multiply(0.5));
            let mut bar = first;
            loop {
                let t0 = view.map.bar_start(bar);
                if x_of(view.map.frames_at(t0).0) > rect.right() {
                    break;
                }
                let t1 = view.map.bar_start(bar + 1);
                let beat = view.map.signature_at(t0).beat_ticks();
                let unit = grid.ticks(view.map, t0);
                let mut t = t0.0 + unit;
                while t < t1.0 {
                    let x = x_of(view.map.frames_at(Ticks(t)).0);
                    if x > rect.right() {
                        break;
                    }
                    if x >= left {
                        let c = if (t - t0.0) % beat == 0 {
                            beat_c
                        } else {
                            part_c
                        };
                        painter.line_segment(
                            [pos2(x, rect.top() + RULER), pos2(x, rect.bottom())],
                            Stroke::new(1.0, c),
                        );
                    }
                    t += unit;
                }
                bar += 1;
            }
        }
        let mut bar = (first - 1) / every * every + 1;
        loop {
            let x = x_of(view.map.frames_at(view.map.bar_start(bar)).0);
            if x > rect.right() {
                break;
            }
            if x >= left {
                painter.line_segment(
                    [pos2(x, rect.top() + 12.0), pos2(x, rect.bottom())],
                    Stroke::new(look.line, tc.grid),
                );
                painter.text(
                    pos2(x + 3.0, rect.top() + RULER - 2.0),
                    Align2::LEFT_BOTTOM,
                    bar.to_string(),
                    FontId::new(look.small, look.numerals_font().family),
                    tc.text_dim,
                );
            }
            bar += every;
        }
        self.draw_time_changes(&painter, view, ruler, &x_of, &look);

        let hover_at = resp.hover_pos();
        // Lanes and clips, under the ruler.
        let lanes = painter.with_clip_rect(Rect::from_min_max(
            pos2(rect.left(), rect.top() + tracks_top),
            rect.max,
        ));
        let links = crate::linked::Links::of(song, &self.selected, self.note_sel.as_deref());
        let mut heads = Vec::with_capacity(song.lanes.len());
        for (i, lane) in song.lanes.iter().enumerate() {
            let top = lane_top(i);
            let name_rect = Rect::from_min_size(pos2(rect.left(), top), vec2(HEADER, lane_h));
            let colour = tc.track(i);
            let head = view.heads.get(i).copied().unwrap_or_default();
            heads.push((i, name_rect, head));
            self.paint_header(&lanes, name_rect, i, lane, head, view, &look);
            lanes.line_segment(
                [
                    pos2(rect.left(), top + lane_h),
                    pos2(rect.right(), top + lane_h),
                ],
                Stroke::new(look.line, tc.line),
            );
            let order = lane
                .clips
                .iter()
                .filter(|c| c.muted)
                .chain(lane.clips.iter().filter(|c| !c.muted));
            for c in order {
                let (x0, x1) = (x_of(c.start), x_of(c.end()));
                if x1 < left || x0 > rect.right() {
                    continue;
                }
                let r = Rect::from_x_y_ranges(
                    x0.max(left)..=x1.min(rect.right()),
                    top + 2.0..=top + lane_h - 2.0,
                );
                let selected = self.selected.contains(&c.id);
                let corner = look.clip_radius.min(r.height() / 2.0).min(r.width() / 2.0);
                if c.muted {
                    lanes.rect_stroke(
                        r,
                        corner,
                        Stroke::new(look.line, tc.clip_border.of(colour).gamma_multiply(0.5)),
                        StrokeKind::Inside,
                    );
                } else {
                    if look.shadow > 0.0 {
                        let drop = (look.shadow / 4.0).min(3.0);
                        lanes.rect_filled(r.translate(vec2(0.0, drop)), corner, tc.shadow);
                    }
                    // Over the lane, so a see-through fill shows it.
                    lanes.rect_filled(r, corner, tc.lane);
                    lanes.rect_filled(r, corner, tc.clip_fill.of(colour));
                    lanes.rect_stroke(
                        r,
                        corner,
                        Stroke::new(look.line, tc.clip_border.of(colour)),
                        StrokeKind::Inside,
                    );
                }
                let shape = c.shape();
                // The source frame under a column: through the stretch.
                let src_at = |x: f32| -> i64 {
                    if c.is_stretched() {
                        c.frame_of_out(view.map, frame_of(x)).round() as i64
                    } else {
                        frame_of(x) - c.start + c.offset as i64
                    }
                };
                let level = |x: f32| {
                    let rel = (src_at(x) - c.offset as i64).clamp(0, c.len as i64 - 1);
                    shape.at(rel as usize, c.len, c.offset)
                };
                if let Some(w) = self.waves.get(&c.source) {
                    let mid = r.center().y + 6.0;
                    let half = (r.height() - 16.0) / 2.0;
                    let wave = if c.muted {
                        colour.gamma_multiply(0.4)
                    } else {
                        colour
                    };
                    let mut cols = Vec::with_capacity(r.width() as usize + 1);
                    let mut x = r.left().floor();
                    while x < r.right() {
                        let f0 = src_at(x);
                        let f1 = src_at(x + 1.0);
                        let lo_src = c.offset as i64;
                        let hi_src = (c.offset + c.len) as i64;
                        let (a, b) = (
                            f0.clamp(lo_src, hi_src) as usize,
                            f1.clamp(lo_src, hi_src) as usize,
                        );
                        if b > a {
                            let (lo, hi) = w.range(a, b);
                            // As it plays: through its fades and volume.
                            let g = level(x);
                            let (lo, hi) = (lo * g, hi * g);
                            cols.push((
                                x + 0.5,
                                mid - hi.min(1.0) * half,
                                mid - lo.max(-1.0) * half - 0.5,
                            ));
                        }
                        x += 1.0;
                    }
                    draw_wave(&lanes, &look, &cols, mid, wave);
                }
                let full = Rect::from_x_y_ranges(x0..=x1, r.y_range());
                // Handles on the clip under the pointer and the selected.
                let handles = selected || hover_at.is_some_and(|h| full.contains(h));
                self.draw_fades(
                    &lanes.with_clip_rect(r),
                    c,
                    full,
                    &x_of,
                    &look,
                    colour,
                    handles,
                );
                if self.show_volume && !c.muted {
                    draw_volume(&lanes.with_clip_rect(r.expand(4.0)), &look, c, full, &x_of);
                }
                if !c.markers.is_empty() {
                    draw_markers(&lanes.with_clip_rect(r), &look, c, view.map, &x_of, colour);
                }
                if let Some(t) = self.words.get(&c.path)
                    && r.height() >= 30.0
                {
                    let ink = if c.muted {
                        tc.text_dim
                    } else {
                        tc.clip_text.of(colour)
                    };
                    draw_words(&lanes.with_clip_rect(r), &look, c, t, view.map, &x_of, ink);
                }
                let mut name = if c.gain_db.abs() >= 0.05 {
                    format!("{} ({:+.1} dB)", c.name(), c.gain_db)
                } else {
                    c.name()
                };
                if let Some(bpm) = c.tempo {
                    name = format!("{name} · {bpm:.0} BPM");
                }
                if c.cents != 0 {
                    name = format!("{name} {}", crate::transpose::label(c.cents));
                }
                let at = r.left_top() + vec2(4.0 + HANDLE, 2.0);
                let font = look.font(look.small);
                if c.muted {
                    lanes.text(at, Align2::LEFT_TOP, name, font, tc.text_dim);
                } else {
                    let g = lanes.layout_no_wrap(name, font, tc.clip_text.of(colour));
                    if let Some(label) = tc.clip_label {
                        let back =
                            Rect::from_min_size(at - vec2(3.0, 1.0), g.size() + vec2(6.0, 2.0));
                        lanes
                            .with_clip_rect(r.intersect(lanes.clip_rect()))
                            .rect_filled(back, 3.0, label);
                    }
                    lanes.galley(at, g, tc.clip_text.of(colour));
                }
                if selected {
                    lanes.rect_stroke(r, corner, Stroke::new(2.0, tc.text), StrokeKind::Inside);
                }
                let key = c.link.as_deref().map(crate::linked::audio_key);
                links.draw(&lanes, key.as_deref(), r, corner, selected);
                if self
                    .focus
                    .as_ref()
                    .is_some_and(|(f, _)| *f == Focus::Clip(c.id))
                {
                    // The keyboard's ring, outside the selection's.
                    lanes.rect_stroke(
                        r.expand(1.0),
                        corner + 1.0,
                        Stroke::new(2.0, tc.accent),
                        StrokeKind::Outside,
                    );
                }
            }
            self.draw_note_clips(
                &lanes,
                &look,
                &links,
                lane,
                top,
                lane_h,
                left,
                rect.right(),
                colour,
                view,
                &x_of,
            );
            if let Some(Some(band)) = view.bands.get(i) {
                let r = Rect::from_min_max(
                    pos2(rect.left(), top + lane_h),
                    pos2(rect.right(), top + lane_h + band.height()),
                );
                if r.bottom() >= rect.top() + tracks_top && r.top() <= rect.bottom() {
                    let tick_x = |t: i64| x_of(view.map.frames_at(Ticks(t)).0);
                    crate::notation::draw_band(&lanes, &look, band, r, left, &tick_x);
                }
                lanes.line_segment(
                    [
                        pos2(rect.left(), r.bottom()),
                        pos2(rect.right(), r.bottom()),
                    ],
                    Stroke::new(look.line, tc.line),
                );
            }
        }
        let heads_clip = Rect::from_min_max(pos2(rect.left(), rect.top() + tracks_top), rect.max);
        self.header_buttons(ui, song, &heads, heads_clip, &mut out);
        if song.lanes.is_empty() {
            painter.text(
                pos2(left + 10.0, rect.top() + tracks_top + 20.0),
                Align2::LEFT_CENTER,
                "No tracks yet",
                look.font(look.body),
                tc.text_dim,
            );
        }

        // Markers, over everything.
        for m in song.markers.iter() {
            let x = x_of(view.map.frames_at(view.map.bar_start(m.bar)).0);
            if x < left - 60.0 || x > rect.right() {
                continue;
            }
            let flag = tc.marker;
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(look.line, flag.gamma_multiply(0.6)),
            );
            let g = painter.layout_no_wrap(m.name.clone(), look.font(look.small), tc.on_accent);
            let label = Rect::from_min_size(pos2(x, rect.top()), g.size() + vec2(8.0, 2.0));
            painter.rect_filled(label, look.radius.min(3.0), flag);
            painter.galley(label.min + vec2(4.0, 1.0), g, tc.on_accent);
            if self
                .focus
                .as_ref()
                .is_some_and(|(f, _)| *f == Focus::Marker(m.id.clone()))
            {
                painter.rect_stroke(
                    label.expand(1.0),
                    3.0,
                    Stroke::new(2.0, tc.text),
                    StrokeKind::Outside,
                );
            }
        }

        // Hit points: dashed lines down through the lanes.
        for h in view.hits {
            let x = x_of(h.at);
            if x >= left && x <= rect.right() {
                lanes.add(egui::Shape::dashed_line(
                    &[pos2(x, rect.top() + RULER), pos2(x, rect.bottom())],
                    Stroke::new(look.line, tc.accent.gamma_multiply(0.7)),
                    4.0,
                    3.0,
                ));
            }
        }

        // Comment pins: a band over what a range covers, and a bubble at
        // the start.
        let hover = resp.hover_pos();
        for p in view.pins {
            let colour = if p.resolved { tc.text_dim } else { tc.comment };
            let x = x_of(p.start);
            let (top, bottom) = match p.lane {
                Some(l) => (lane_top(l), lane_top(l) + lane_h),
                None => (rect.top() + RULER, rect.bottom()),
            };
            if let Some(e) = p.end {
                let band = Rect::from_x_y_ranges(x.max(left)..=x_of(e).max(left), top..=bottom);
                let alpha = if p.selected { 0.22 } else { 0.1 };
                lanes.rect_filled(band, 0.0, colour.gamma_multiply(alpha));
            }
            let line = Stroke::new(if p.selected { 2.0 } else { 1.0 }, colour);
            lanes.line_segment([pos2(x, top), pos2(x, bottom)], line);
            let bubble_top = match p.lane {
                Some(_) => top,
                None => rect.top() + RULER - PIN.y,
            };
            let r = pin_rect(x, bubble_top);
            let on = if p.lane.is_some() { &lanes } else { &painter };
            on.rect_filled(r, 3.0, colour);
            if p.selected {
                on.rect_stroke(r, 3.0, Stroke::new(1.5, tc.text), StrokeKind::Outside);
            }
            if hover.is_some_and(|h| r.expand(2.0).contains(h)) {
                let g =
                    painter.layout_no_wrap(p.text.clone(), look.font(look.small + 1.0), tc.text);
                let tip =
                    Rect::from_min_size(r.right_top() + vec2(4.0, 0.0), g.size() + vec2(8.0, 4.0));
                painter.rect_filled(tip, look.radius.min(6.0), tc.raised);
                painter.rect_stroke(
                    tip,
                    look.radius.min(6.0),
                    Stroke::new(look.line, colour),
                    StrokeKind::Inside,
                );
                painter.galley(tip.min + vec2(4.0, 2.0), g, tc.text);
            }
        }

        // Playhead: a line with a head on the ruler, glowing if the
        // theme says so.
        let px = x_of(view.playhead);
        if px >= left && px <= rect.right() {
            let line = [pos2(px, rect.top()), pos2(px, rect.bottom())];
            if let Some(radius) = look.glow {
                for k in 1..=4 {
                    let w = 1.5 + radius * k as f32 / 2.0;
                    let a = 0.22 * (1.0 - k as f32 / 5.0);
                    painter.line_segment(line, Stroke::new(w, tc.glow.gamma_multiply(a)));
                }
            }
            painter.line_segment(line, Stroke::new(1.5 * look.line, tc.playhead));
            let top = rect.top() + RULER - 9.0;
            painter.add(egui::Shape::convex_polygon(
                vec![
                    pos2(px - 5.0, top),
                    pos2(px + 5.0, top),
                    pos2(px, top + 8.0),
                ],
                tc.playhead,
                Stroke::NONE,
            ));
        }

        // Right-click menus.
        let end_bar = Self::end_bar(view.map, song);
        resp.context_menu(|ui| match self.menu {
            Some((Hit::Clip { id, zone }, at)) => {
                // ai: split
                if ui.button("Split here").clicked() {
                    out.changed |= self.split_at(song, snap_to(at));
                    ui.close();
                }
                // ai: delete
                if ui.button("Delete").clicked() {
                    out.changed |= self.delete_selected(song);
                    ui.close();
                }
                ui.separator();
                // ai: clip-fades
                if ui.button("Fades and volume…").clicked() {
                    self.fades_open = true;
                    ui.close();
                }
                for (end, name) in [(false, "Fade in shape"), (true, "Fade out shape")] {
                    ui.menu_button(name, |ui| {
                        for c in Curve::ALL {
                            // ai: edit_clip_volume.fade_in_curve
                            if ui.button(curve_label(c)).clicked() {
                                out.changed |= self.set_curves(song, end, c);
                                ui.close();
                            }
                        }
                    });
                }
                // ai: crossfade
                if ui.button("Crossfade where they meet").clicked() {
                    out.changed |= self.crossfade_selected(song, view.rate);
                    ui.close();
                }
                ui.separator();
                let follows = (song.lanes.iter())
                    .flat_map(|l| l.clips.iter())
                    .filter(|c| self.selected.contains(&c.id))
                    .all(|c| c.tempo.is_some());
                let mut follow = follows;
                if ui
                    // ai: follow-tempo
                    .checkbox(&mut follow, "Follow tempo")
                    .on_hover_text(
                        "The clip keeps to the song's beats, stretched to the tempo map",
                    )
                    .clicked()
                {
                    out.command = Some(Cmd::FollowTempo);
                    ui.close();
                }
                // ai: clip-tempo
                if ui.button("Clip tempo…").clicked() {
                    out.command = Some(Cmd::ClipTempo);
                    ui.close();
                }
                // ai: transpose-clips
                if ui.button("Transpose…").clicked() {
                    out.command = Some(Cmd::TransposeClips);
                    ui.close();
                }
                let link = song.find(id).and_then(|c| c.link.as_deref()).map(crate::linked::audio_key);
                let links = crate::linked::Links::of(song, &self.selected, None);
                let n = links.count(link.as_deref());
                if n > 1 {
                    // ai: unlink_clip
                    if ui
                        .button("Unlink")
                        .on_hover_text(format!(
                            "This clip is one of {n} linked copies: changing its gain, volume \
                             line, transpose or pitch edits changes them all. Unlink it to \
                             change it on its own"
                        ))
                        .clicked()
                    {
                        out.changed |= crate::linked::unlink(song, Some(id), None);
                        ui.close();
                    }
                }
                if let Zone::Marker(i) = zone {
                    // ai: stretch_clips.remove_markers
                    if ui.button("Delete stretch marker").clicked() {
                        if let Some(c) = song.find_mut(id)
                            && i < c.markers.len()
                        {
                            c.markers.remove(i);
                            c.retime(view.map);
                            out.changed = true;
                        }
                        ui.close();
                    }
                } else if ui
                    // ai: stretch_clips.add_markers
                    .button("Add stretch marker here")
                    .on_hover_text("Drag it onto a beat to move the attack under it")
                    .clicked()
                {
                    // On the clip and those edited with it, at this frame.
                    let ids = groups::with_partners(song, self.grouped(), [id]);
                    for c in (song.lanes.iter_mut())
                        .flat_map(|l| l.clips.iter_mut())
                        .filter(|c| ids.contains(&c.id) && at > c.start && at < c.end())
                    {
                        let frame = c.frame_of_out(view.map, at).round() as i64;
                        if c.add_marker(frame).is_some() {
                            c.retime(view.map);
                            out.changed = true;
                        }
                    }
                    ui.close();
                }
                // ai: stretch-markers-from-attacks
                if ui.button("Stretch markers from attacks").clicked() {
                    out.command = Some(Cmd::StretchMarkersFromAttacks);
                    ui.close();
                }
                // ai: clear-stretch-markers
                if ui.button("Clear stretch markers").clicked() {
                    out.command = Some(Cmd::ClearStretchMarkers);
                    ui.close();
                }
                // ai: fix-timing
                if ui.button("Fix timing…").clicked() {
                    out.command = Some(Cmd::FixTiming);
                    ui.close();
                }
                // ai: quantise-audio
                if ui.button("Quantise audio…").clicked() {
                    out.command = Some(Cmd::QuantiseAudio);
                    ui.close();
                }
                // ai: tempo-from-clip
                if ui.button("Tempo from performance…").clicked() {
                    out.command = Some(Cmd::TempoFromClip);
                    ui.close();
                }
                // ai: beat-grid
                if ui.button("Find beat grid…").clicked() {
                    out.command = Some(Cmd::BeatGrid);
                    ui.close();
                }
                // ai: anchor-to-beat
                if ui.button("Line up playhead point with nearest beat").clicked() {
                    out.command = Some(Cmd::AnchorToBeat);
                    ui.close();
                }
                // ai: anchor-to-bar
                if ui.button("Line up playhead point with nearest bar").clicked() {
                    out.command = Some(Cmd::AnchorToBar);
                    ui.close();
                }
                if let Zone::Volume(Some(i)) = zone
                    // ai: edit_clip_volume.points
                    && ui.button("Delete volume point").clicked()
                {
                    if let Some(c) = song.find_mut(id)
                        && i < c.envelope.len()
                    {
                        c.envelope.remove(i);
                        out.changed = true;
                    }
                    ui.close();
                }
                if !self.simple {
                    ui.separator();
                    // ai: process_clips.how
                    if ui.button("Normalise").clicked() {
                        out.process = Some(Process::Normalise);
                        ui.close();
                    }
                    // ai: process_clips.how
                    if ui.button("Reverse").clicked() {
                        out.process = Some(Process::Reverse);
                        ui.close();
                    }
                    ui.menu_button("Change gain", |ui| {
                        for db in GAIN_STEPS {
                            // ai: process_clips.how
                            if ui.button(Process::Gain(db).label()).clicked() {
                                out.process = Some(Process::Gain(db));
                                ui.close();
                            }
                        }
                    });
                }
                ui.separator();
                // ai: copy
                if ui.button("Copy").clicked() {
                    out.copy = true;
                    ui.close();
                }
                if ui
                    // ai: glue
                    .button("Glue into one clip")
                    .on_hover_text(
                        "The selected clips on a track become one clip that moves, loops and \
                         fades as one piece; one clip selected is glued to the next",
                    )
                    .clicked()
                {
                    out.glue = true;
                    ui.close();
                }
                if ui
                    // ai: bounce
                    .button("Bounce to a new track")
                    .on_hover_text(
                        "The selected clips through their track's effects, as new audio on a \
                         track beside it; they are muted where they are",
                    )
                    .clicked()
                {
                    out.bounce = true;
                    ui.close();
                }
                ui.separator();
                // ai: add_comment.track
                if ui.button("Add comment here…").clicked() {
                    out.comment_at = Some((self.menu_lane, snap_to(at)));
                    ui.close();
                }
            }
            Some((Hit::NoteClip { lane, index, .. }, _)) => {
                let key = (song.lanes.get(lane))
                    .and_then(|l| l.notes.get(index))
                    .map(|c| c.id.clone());
                if let Some(key) = key {
                    let linked = song.lanes[lane].notes[index].pattern.is_some();
                    if linked {
                        // ai: pattern-editor
                        if ui.button("Edit pattern…").clicked() {
                            self.note_sel = Some(key.clone());
                            out.pattern = Some(PatternAsk::Edit(key.clone()));
                            ui.close();
                        }
                        if ui
                            // ai: edit_clip.pattern_unique
                            .button("Make unique")
                            .on_hover_text(
                                "Give this clip a copy of the pattern of its own, so editing it \
                                 leaves the other clips playing the pattern as they are",
                            )
                            .clicked()
                        {
                            out.pattern = Some(PatternAsk::Unique(key.clone()));
                            ui.close();
                        }
                        if ui
                            // ai: pattern-to-notes
                            .button("Convert to plain notes")
                            .on_hover_text(
                                "Keep the notes the pattern plays here as notes to edit one by \
                                 one, no longer following the pattern",
                            )
                            .clicked()
                        {
                            out.pattern = Some(PatternAsk::ToNotes(key.clone()));
                            ui.close();
                        }
                    // ai: notes-editor
                    } else if ui.button("Edit notes…").clicked() {
                        self.note_sel = Some(key.clone());
                        out.edit_notes = Some(key.clone());
                        ui.close();
                    }
                    let muted = song.lanes[lane].notes[index].muted;
                    // ai: edit_clip.muted
                    if ui.button(if muted { "Unmute" } else { "Mute" }).clicked() {
                        song.lanes[lane].notes[index].muted = !muted;
                        out.changed = true;
                        ui.close();
                    }
                    if !linked {
                        let c = &mut song.lanes[lane].notes[index];
                        let mut looping = c.loop_len.is_some();
                        // ai: edit_clip
                        if ui
                            .checkbox(&mut looping, "Loop notes")
                            .on_hover_text(
                                "Play the notes from the clip's start to its end as it is now \
                                 over and over: drag its end out to repeat them, and editing \
                                 them changes every time round",
                            )
                            .changed()
                        {
                            c.loop_len = looping.then_some(c.offset + c.len);
                            out.changed = true;
                            ui.close();
                        }
                    }
                    let group = crate::linked::note_key(&song.lanes[lane].notes[index]);
                    let n = crate::linked::Links::of(song, &self.selected, None)
                        .count(group.as_deref());
                    if n > 1 && !linked {
                        // ai: unlink_clip
                        if ui
                            .button("Unlink")
                            .on_hover_text(format!(
                                "This clip is one of {n} linked copies: editing its notes \
                                 changes them all. Unlink it to edit it on its own"
                            ))
                            .clicked()
                        {
                            out.changed |= crate::linked::unlink(song, None, Some(&key));
                            ui.close();
                        }
                    }
                    // ai: duplicate_clip
                    let dup = ui.button("Duplicate (linked)").on_hover_text(if linked {
                        "A copy after it playing the same pattern: editing the pattern \
                         changes both"
                    } else {
                        "A linked copy after it: editing the notes of either changes both"
                    });
                    let mut copy = None;
                    if dup.clicked() {
                        copy = Some(true);
                    }
                    // ai: duplicate_clip
                    if !linked
                        && ui
                            .button("Duplicate as a separate copy")
                            .on_hover_text("A copy after it with notes of its own")
                            .clicked()
                    {
                        copy = Some(false);
                    }
                    if let Some(link) = copy {
                        let src = &mut song.lanes[lane].notes[index];
                        let shared = (link && src.pattern.is_none())
                            .then(|| src.link.get_or_insert_with(daw_project::new_id).clone());
                        let mut c = src.clone();
                        c.id = daw_project::new_id();
                        c.start = c.end();
                        c.link = shared;
                        if c.link.is_none() {
                            for n in &mut c.notes {
                                n.id = daw_project::new_id();
                            }
                        }
                        self.note_sel = Some(c.id.clone());
                        song.lanes[lane].notes.push(c);
                        out.changed = true;
                        ui.close();
                    }
                    let after = (song.lanes[lane].notes.iter())
                        .filter(|n| !n.muted && n.id != key)
                        .filter(|n| n.start >= song.lanes[lane].notes[index].start)
                        .min_by_key(|n| n.start)
                        .map(|n| n.id.clone());
                    if let Some(next) = after.filter(|_| !linked)
                        && ui
                            // ai: glue_clips
                            .button("Glue to the next clip")
                            .on_hover_text(
                                "This clip and the next one on the track become one clip, \
                                 holding the notes both played",
                            )
                            .clicked()
                    {
                        match daw_project::glue_note_clips(song.lanes[lane].notes, &[&key, &next]) {
                            Ok(id) => {
                                self.note_sel = Some(id);
                                out.changed = true;
                            }
                            Err(e) => out.message = Some(e),
                        }
                        ui.close();
                    }
                    // ai: delete_clip
                    if ui.button("Delete").clicked() {
                        song.lanes[lane].notes.remove(index);
                        self.note_sel = None;
                        out.changed = true;
                        ui.close();
                    }
                }
            }
            Some((Hit::Header(l), _)) if l < song.lanes.len() => {
                let lane = &song.lanes[l];
                ui.label(egui::RichText::new(lane.name).strong());
                ui.separator();
                if lane.kind.plays_notes() {
                    // ai: instrument-window
                    if ui.button("Instrument…").clicked() {
                        out.track = Some((l, TrackAsk::Instrument));
                        ui.close();
                    }
                }
                // ai: view
                if ui
                    .button("Effects…")
                    .on_hover_text("Its effect chain, with presets")
                    .clicked()
                {
                    out.track = Some((l, TrackAsk::Effects));
                    ui.close();
                }
                ui.separator();
                track_entries(ui, l, lane.kind, view, &mut out);
                let on = format!("Add comment on {}…", lane.name);
                // ai: add_comment
                if ui.button(on).clicked() {
                    out.comment_at = Some((Some(l), snap_to(view.playhead)));
                    ui.close();
                }
            }
            Some((Hit::Pin(i), _)) if i < view.pins.len() => {
                // ai: view
                if ui.button("Show comment").clicked() {
                    out.pin = Some(view.pins[i].id.clone());
                    ui.close();
                }
            }
            Some((Hit::Marker(i), _)) if i < song.markers.len() => {
                // ai: set_loop.from_bar
                if ui.button("Loop this section").clicked() {
                    out.loop_bars = Some(section(song.markers, i, end_bar));
                    ui.close();
                }
                // ai: edit_lesson_notes.add_tabs
                if ui.button("Add tab for this section…").clicked() {
                    out.tab_for = Some(song.markers[i].id.clone());
                    ui.close();
                }
                // ai: edit_markers.change
                if ui.button("Rename…").clicked() {
                    self.rename = Some((i, song.markers[i].name.clone()));
                    self.rename_focused = false;
                    ui.close();
                }
                // ai: edit_markers.remove
                if ui.button("Delete marker").clicked() {
                    song.markers.remove(i);
                    ui.close();
                }
            }
            Some((_, at)) => {
                if let Some(l) = self.menu_lane
                    && let Some(lane) = song.lanes.get(l)
                {
                    let audio = lane.kind == daw_project::TrackKind::Audio;
                    if audio {
                        // ai: paste_clips.track
                        if ui
                            .add_enabled(view.can_paste, egui::Button::new("Paste here"))
                            .clicked()
                        {
                            out.paste = Some((l, snap_to(at), false));
                            ui.close();
                        }
                        if ui
                            .add_enabled(
                                view.can_paste,
                                // ai: paste_clips.rendered
                                egui::Button::new("Paste here as rendered audio"),
                            )
                            .on_hover_text(
                                "The copied clips through their own tracks' effects, as one new clip",
                            )
                            .clicked()
                        {
                            out.paste = Some((l, snap_to(at), true));
                            ui.close();
                        }
                    }
                    track_entries(ui, l, lane.kind, view, &mut out);
                    ui.separator();
                }
                let bar = Self::bar_at(view.map, snap(view.map, at, Grid::Bar));
                // ai: edit_markers.add
                if ui.button(format!("Add marker at bar {bar}")).clicked() {
                    self.add_marker(song, bar);
                    ui.close();
                }
                if self.menu_lane.is_none() {
                    let bb = view.map.bar_beat(view.map.ticks_at(Frames(snap(
                        view.map,
                        at,
                        Grid::Beat,
                    ))));
                    let (b, beat) = (bb.bar, bb.beat as u32);
                    if (b, beat) != (1, 1)
                        && ui
                            // ai: set_tempo.changes
                            .button(format!("Add tempo change at bar {b}, beat {beat}…"))
                            .clicked()
                    {
                        out.tempo = Some(crate::tempo::Ask::Tempo(b, beat));
                        ui.close();
                    }
                    if bar > 1
                        && ui
                            // ai: set_tempo.meters
                            .button(format!("Add time signature change at bar {bar}…"))
                            .clicked()
                    {
                        out.tempo = Some(crate::tempo::Ask::Meter(bar));
                        ui.close();
                    }
                    // ai: view
                    if ui.button("Tempo and time signature…").clicked() {
                        out.tempo = Some(crate::tempo::Ask::Open);
                        ui.close();
                    }
                    // ai: hit-point
                    if ui.button("Add hit point here").clicked() {
                        out.hit = Some(crate::hitpoints::HitAsk::Add(at));
                        ui.close();
                    }
                    ui.separator();
                    for c in [Cmd::Sections, Cmd::TimeWindow, Cmd::Loop, Cmd::Metronome] {
                        // ai: sections
                        if ui.button(c.info().name).clicked() {
                            out.command = Some(c);
                            ui.close();
                        }
                    }
                }
                if let Some(l) = self.menu_lane.filter(|&l| song.lanes[l].kind.plays_notes())
                    // ai: add_note_clip.track
                    && ui.button("Add note clip here").clicked()
                {
                    let n = song.lanes.iter().map(|l| l.notes.len()).sum::<usize>() + 1;
                    let c = new_note_clip(view.map, snap_to(at), n);
                    self.note_sel = Some(c.id.clone());
                    out.edit_notes = Some(c.id.clone());
                    song.lanes[l].notes.push(c);
                    out.changed = true;
                    ui.close();
                }
                if let Some(l) = self.menu_lane.filter(|&l| song.lanes[l].kind.plays_notes())
                    && ui
                        // ai: edit_pattern.new
                        .button("Add drum pattern here")
                        .on_hover_text("A clip playing a new pattern of steps, to edit in the pattern editor")
                        .clicked()
                {
                    out.pattern = Some(PatternAsk::Add(l, snap_to(at)));
                    ui.close();
                }
                let on = match self.menu_lane.and_then(|l| song.lanes.get(l)) {
                    Some(l) => format!("Add comment on {} here…", l.name),
                    None => "Add comment on the whole song here…".into(),
                };
                // ai: add_comment.track
                if ui.button(on).clicked() {
                    out.comment_at = Some((self.menu_lane, snap_to(at)));
                    ui.close();
                }
            }
            None => {
                ui.close();
            }
        });

        if self.fades_open {
            self.fades_window(ui.ctx(), song, view, &mut out);
        }

        // Renaming a marker.
        if let Some((i, mut text)) = self.rename.take() {
            let mut keep = i < song.markers.len();
            let mut done = false;
            egui::Window::new("Rename marker")
                .collapsible(false)
                .resizable(false)
                .default_pos(rect.left_top() + vec2(HEADER + 40.0, RULER + 10.0))
                .show(ui.ctx(), |ui| {
                    let enter = ui.input(|i| i.key_pressed(Key::Enter));
                    let r = ui
                        // ai: edit_markers.change
                        .text_edit_singleline(&mut text)
                        .accessible_name("Marker name");
                    if !self.rename_focused {
                        r.request_focus();
                        self.rename_focused = true;
                    }
                    ui.horizontal(|ui| {
                        // ai: edit_markers.change
                        done = ui.button("OK").clicked() || (enter && r.lost_focus());
                        // ai: view
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                });
            if keep && done {
                let name = text.trim().to_string();
                if !name.is_empty() && song.markers[i].name != name {
                    song.markers[i].name = name;
                }
            } else if keep {
                self.rename = Some((i, text));
            }
            if self.rename.is_none() {
                self.rename_focused = false;
            }
        }
        out
    }
}

/// A track's own right-click entries: on its header, and on an empty
/// place in its lane.
fn track_entries(
    ui: &mut egui::Ui,
    l: usize,
    kind: daw_project::TrackKind,
    view: &View,
    out: &mut Outcome,
) {
    let audio = kind == daw_project::TrackKind::Audio;
    let (can, frozen) = view.freeze.get(l).copied().unwrap_or_default();
    let render = if audio {
        "Render in place"
    } else {
        "Render group in place"
    };
    if ui
        // ai: render_track.track
        .button(render)
        .on_hover_text(
            "Its audio through its effects, before its fader, as a new track \
                 beside it; this one is kept, muted",
        )
        .clicked()
    {
        out.track = Some((l, TrackAsk::RenderInPlace));
        ui.close();
    }
    if frozen {
        // ai: freeze_track.frozen
        if ui.button("Unfreeze").clicked() {
            out.track = Some((l, TrackAsk::Freeze));
            ui.close();
        }
    } else if ui
        // ai: freeze_track.frozen
        .add_enabled(can, egui::Button::new("Freeze"))
        .on_hover_text(
            "Play it from audio rendered through its effects, with them switched \
                 off, to save computer power",
        )
        .on_disabled_hover_text("It has no effects switched on to freeze")
        .clicked()
    {
        out.track = Some((l, TrackAsk::Freeze));
        ui.close();
    }
    let head = view.heads.get(l).copied().unwrap_or_default();
    if ui
        // ai: track-off
        .button(if head.off { "Turn on" } else { "Turn off" })
        .on_hover_text(if head.off {
            "Play it again, with its instrument and effects"
        } else {
            "Keep it in the project but play nothing, so its instrument and effects \
             use no computer power"
        })
        .clicked()
    {
        out.track = Some((l, TrackAsk::Off));
        ui.close();
    }
    let mut safe = head.solo_safe;
    if ui
        // ai: solo-safe
        .checkbox(&mut safe, "Solo-safe")
        .on_hover_text("Stay heard when other tracks are soloed, as a reverb bus or a click should")
        .clicked()
    {
        out.track = Some((l, TrackAsk::SoloSafe));
        ui.close();
    }
    if ui
        // ai: save_preset.kind
        .button("Save track as preset…")
        .on_hover_text("Keep its instrument, effects, sends and settings to use again")
        .clicked()
    {
        out.track = Some((l, TrackAsk::SavePreset));
        ui.close();
    }
    if !kind.is_bus() {
        let ask = |a| Some((l, TrackAsk::Version(a)));
        ui.menu_button("Versions", |ui| {
            let (names, at) = view.versions.get(l).cloned().unwrap_or_default();
            for (i, n) in names.iter().enumerate() {
                // ai: switch_version.version
                if ui.radio(i == at, n).clicked() {
                    out.track = ask(VersionAsk::Switch(i));
                    ui.close();
                }
            }
            if !names.is_empty() {
                ui.separator();
            }
            if ui
                // ai: duplicate-track-version
                .button("Duplicate this version")
                .on_hover_text("A copy of what the track plays, to change and compare")
                .clicked()
            {
                out.track = ask(VersionAsk::Duplicate);
                ui.close();
            }
            if ui
                // ai: new-track-version
                .button("New empty version")
                .on_hover_text(
                    "Same instrument, effects and fader, no clips, notes or \
                         automation",
                )
                .clicked()
            {
                out.track = ask(VersionAsk::New);
                ui.close();
            }
            // ai: track-versions
            if ui.button("Track versions…").clicked() {
                out.track = ask(VersionAsk::Window);
                ui.close();
            }
        });
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use daw_engine::time::{Tempo, TimeSignature};

    const RATE: u32 = 48_000;

    pub(crate) fn source(frames: usize) -> Arc<StreamSource> {
        let dir = std::env::temp_dir().join(format!("daw-app-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // One file each: tests run in parallel.
        let path = dir.join(format!("{}.wav", next_id()));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..frames {
            w.write_sample(0i16).unwrap();
        }
        w.finalize().unwrap();
        StreamSource::open(&path).unwrap()
    }

    pub(crate) fn clip(start: i64, frames: usize) -> Clip {
        let s = source(frames);
        let path = s.path.clone();
        Clip::new(s, path, start, None)
    }

    #[test]
    fn trimming_keeps_the_audio_in_place_and_within_the_file() {
        let mut c = clip(1000, 10_000);
        c.trim_start(3000);
        assert_eq!(
            (c.start, c.offset, c.len, c.end()),
            (3000, 2000, 8000, 11_000)
        );
        // Back past the file's start stops at it.
        c.trim_start(0);
        assert_eq!((c.start, c.offset, c.len), (1000, 0, 10_000));
        c.trim_end(5000);
        assert_eq!(c.end(), 5000);
        // Past the file's end stops at it.
        c.trim_end(50_000);
        assert_eq!(c.end(), 11_000);
        // Never shorter than MIN_LEN.
        c.trim_start(20_000);
        assert_eq!(c.len, MIN_LEN);
        assert_eq!(c.end(), 11_000);
    }

    #[test]
    fn a_take_before_the_start_is_trimmed() {
        let c = clip(-500, 2000);
        assert_eq!((c.start, c.offset, c.len), (0, 500, 1500));
    }

    #[test]
    fn splitting_makes_two_clips_that_play_the_same() {
        let c = clip(1000, 10_000);
        let (a, b) = c.split(4000).unwrap();
        assert_eq!((a.start, a.offset, a.len), (1000, 0, 3000));
        assert_eq!((b.start, b.offset, b.len), (4000, 3000, 7000));
        assert_eq!(a.id, c.id);
        assert_ne!(b.id, c.id);
        assert!(c.split(1010).is_none(), "too near the start");
        assert!(c.split(20_000).is_none(), "outside");
    }

    #[test]
    fn a_new_take_mutes_the_takes_under_it_and_takes_can_be_chosen() {
        let mut clips = vec![clip(0, 48_000)]; // an imported backing track
        add_take(&mut clips, clip(1000, 10_000));
        add_take(&mut clips, clip(5000, 10_000));
        add_take(&mut clips, clip(30_000, 1000)); // elsewhere: nothing under it
        assert_eq!(take_count(&clips), 3);
        let muted: Vec<bool> = clips.iter().map(|c| c.muted).collect();
        assert_eq!(muted, vec![false, true, false, false]);
        assert_eq!(active_take(&clips), Some(2));

        choose_take(&mut clips, Some(0));
        let muted: Vec<bool> = clips.iter().map(|c| c.muted).collect();
        assert_eq!(muted, vec![false, false, true, false]);

        choose_take(&mut clips, None);
        let muted: Vec<bool> = clips.iter().map(|c| c.muted).collect();
        assert_eq!(muted, vec![false, true, true, true], "the file still plays");
    }

    #[test]
    fn snapping_goes_to_the_nearest_beat_or_bar() {
        // 120 BPM in 4/4: a beat is 24,000 frames, a bar 96,000.
        let map = TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON);
        assert_eq!(snap(&map, 35_000, Grid::Beat), 24_000);
        assert_eq!(snap(&map, 37_000, Grid::Beat), 48_000);
        assert_eq!(snap(&map, 47_000, Grid::Bar), 0);
        assert_eq!(snap(&map, 49_000, Grid::Bar), 96_000);
        // Half a beat is 12,000 frames; a triplet 8,000.
        assert_eq!(snap(&map, 17_000, Grid::Part(2)), 12_000);
        assert_eq!(snap(&map, 13_000, Grid::Part(3)), 16_000);
        assert_eq!(snap(&map, 4_000, Grid::Part(8)), 3_000);
    }

    #[test]
    fn the_grid_follows_tempo_and_time_signature_changes() {
        // 120 BPM to bar 3, then 60 BPM; 3/4 from bar 5.
        let mut map = TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON);
        map.add_tempo(map.bar_start(3), Tempo::bpm(60)).unwrap();
        map.add_meter(map.bar_start(5), TimeSignature::new(3, 4))
            .unwrap();
        let bar3 = map.frames_at(map.bar_start(3)).0;
        assert_eq!(bar3, 192_000);
        // Beats are a second long after the change.
        assert_eq!(snap(&map, bar3 + 30_000, Grid::Beat), bar3 + 48_000);
        assert_eq!(step(&map, bar3, 3, Grid::Part(2)), bar3 + 72_000);
        let bar5 = map.frames_at(map.bar_start(5)).0;
        assert_eq!(step(&map, bar5, 1, Grid::Bar), bar5 + 3 * 48_000);
        // Auto picks a finer step the further in it zooms.
        assert_eq!(Grid::Auto.at_zoom(&map, 0, 4.0), Grid::Bar);
        assert_eq!(Grid::Auto.at_zoom(&map, 0, 40.0), Grid::Beat);
        assert_eq!(Grid::Auto.at_zoom(&map, 0, 400.0), Grid::Part(8));
        // At the slower tempo a beat is wider, so finer at the same zoom.
        assert_eq!(Grid::Auto.at_zoom(&map, bar3, 30.0), Grid::Part(2));
        assert_eq!(Grid::Beat.at_zoom(&map, 0, 400.0), Grid::Beat);
    }

    #[test]
    fn fades_stay_inside_the_clip_through_trims_and_splits() {
        let mut c = clip(0, 10_000);
        c.set_fade(false, 3000);
        c.set_fade(true, 9000);
        assert_eq!((c.fade_in.len, c.fade_out.len), (3000, 7000));
        c.trim_end(5000);
        assert_eq!(c.fade_in.len + c.fade_out.len, 5000);
        assert_eq!((c.fade_in.len, c.fade_out.len), (1500, 3500));
        let (a, b) = c.split(2500).unwrap();
        assert_eq!((a.fade_in.len, a.fade_out.len), (1500, 0), "a clean cut");
        assert_eq!((b.fade_in.len, b.fade_out.len), (0, 2500));
    }

    #[test]
    fn slipping_moves_the_audio_inside_the_clip() {
        let mut c = clip(1000, 10_000);
        c.trim_start(3000);
        c.trim_end(6000);
        assert_eq!((c.start, c.offset, c.len), (3000, 2000, 3000));
        c.slip_to(500);
        assert_eq!((c.start, c.offset, c.len), (3000, 500, 3000));
        c.slip_to(-50);
        assert_eq!(c.offset, 0, "no further than the file's start");
        c.slip_to(99_999);
        assert_eq!(c.offset, 7000, "or its end");
    }

    #[test]
    fn volume_points_stay_with_the_audio() {
        let mut c = clip(1000, 10_000);
        c.trim_start(2000); // offset 1000
        let i = c.add_point(500, -6.0);
        assert_eq!(c.envelope[i].at, 1500, "a source frame");
        c.add_point(2500, 0.0);
        assert_eq!(c.envelope_db(1500), -3.0);
        assert_eq!(c.add_point(500, -100.0), 0, "the same frame moves it");
        assert_eq!(c.envelope[0].db, MIN_DB);
        // Slipped, the line moves with the audio.
        c.slip_to(0);
        assert_eq!(c.envelope_db(2500), -24.0);
    }

    #[test]
    fn crossfades_cover_overlaps_and_joins() {
        // Overlapping by 2000 frames.
        let (mut a, mut b) = (clip(0, 10_000), clip(8000, 10_000));
        assert!(crossfade(&mut a, &mut b, 960));
        assert_eq!((a.fade_out.len, b.fade_in.len), (2000, 2000));
        assert_eq!(a.fade_out.curve, Curve::EqualPower);
        // Touching, each with audio to spare: stretched into each other.
        let mut a = clip(0, 10_000);
        a.trim_end(5000);
        let mut b = clip(0, 10_000);
        b.trim_start(5000);
        assert!(crossfade(&mut b, &mut a, 960), "either order");
        assert_eq!((a.end(), b.start), (5480, 4520));
        assert_eq!((a.fade_out.len, b.fade_in.len), (960, 960));
        // Far apart, or one inside the other: nothing.
        let (mut a, mut b) = (clip(0, 1000), clip(5000, 1000));
        assert!(!crossfade(&mut a, &mut b, 960));
        let (mut a, mut b) = (clip(0, 10_000), clip(2000, 1000));
        assert!(!crossfade(&mut a, &mut b, 960));
    }

    #[test]
    fn ripple_delete_closes_only_empty_time() {
        // A clip at 3000..4000 went; another clip covers part of its time.
        let mut clips = vec![clip(0, 1000), clip(6000, 1000)];
        let mut under = clip(2500, 1000);
        under.id = 99;
        clips.push(under);
        close_gaps(&mut clips, &[(3000, 4000)]);
        let starts: Vec<i64> = clips.iter().map(|c| c.start).collect();
        // 3500..4000 was empty: 500 frames close.
        assert_eq!(starts, vec![0, 5500, 2500]);
    }

    #[test]
    fn deleting_and_trimming_ripple_when_asked() {
        let mut clips = vec![clip(0, 1000), clip(1000, 1000), clip(2000, 1000)];
        let mut markers = Vec::new();
        let mut tl = Timeline {
            ripple: true,
            ..Timeline::default()
        };
        let mut song = Song {
            lanes: vec![Lane {
                name: "Guitar",
                clips: &mut clips,
                kind: daw_project::TrackKind::Audio,
                in_folder: false,
                notes: Box::leak(Box::default()),
            }],
            markers: &mut markers,
        };
        tl.selected.insert(song.lanes[0].clips[1].id);
        assert!(tl.delete_selected(&mut song));
        let starts: Vec<i64> = song.lanes[0].clips.iter().map(|c| c.start).collect();
        assert_eq!(starts, vec![0, 1000]);

        // Trimming the first clip's end a beat shorter pulls the next in.
        let map = TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON);
        let mut clips = vec![clip(0, 96_000), clip(96_000, 10_000)];
        let mut song = Song {
            lanes: vec![Lane {
                name: "Guitar",
                clips: &mut clips,
                kind: daw_project::TrackKind::Audio,
                in_folder: false,
                notes: Box::leak(Box::default()),
            }],
            markers: &mut markers,
        };
        let id = song.lanes[0].clips[0].id;
        tl.focus = Some((Focus::Clip(id), egui::Id::NULL));
        assert!(tl.trim_focused(&mut song, &map, -1, Grid::Beat, true));
        assert_eq!(song.lanes[0].clips[0].end(), 72_000);
        assert_eq!(song.lanes[0].clips[1].start, 72_000);
    }

    #[test]
    fn grouped_tracks_are_edited_together() {
        let map = TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON);
        let mut kick = vec![clip(0, 96_000)];
        let mut snare = vec![clip(0, 96_000)];
        let mut bass = vec![clip(0, 96_000)];
        let mut markers = Vec::new();
        let mut tl = Timeline {
            groups: vec![LaneGroup {
                name: "Drums".into(),
                lanes: vec![0, 1],
            }],
            ..Timeline::default()
        };
        let mut song = Song {
            lanes: vec![
                Lane {
                    name: "Kick",
                    clips: &mut kick,
                    kind: daw_project::TrackKind::Audio,
                    in_folder: false,
                    notes: Box::leak(Box::default()),
                },
                Lane {
                    name: "Snare",
                    clips: &mut snare,
                    kind: daw_project::TrackKind::Audio,
                    in_folder: false,
                    notes: Box::leak(Box::default()),
                },
                Lane {
                    name: "Bass",
                    clips: &mut bass,
                    kind: daw_project::TrackKind::Audio,
                    in_folder: false,
                    notes: Box::leak(Box::default()),
                },
            ],
            markers: &mut markers,
        };
        let k = song.lanes[0].clips[0].id;
        let sn = song.lanes[1].clips[0].id;

        // Selecting one selects its partner, but not the bass.
        tl.select_only(&song, k);
        assert_eq!(tl.selected, HashSet::from([k, sn]));
        tl.toggle_selected(&song, sn);
        assert!(tl.selected.is_empty());

        // Split at a beat: both drum tracks split, the bass doesn't.
        tl.select_only(&song, k);
        assert!(tl.split_at(&mut song, 24_000));
        let lens: Vec<usize> = song.lanes.iter().map(|l| l.clips.len()).collect();
        assert_eq!(lens, [2, 2, 1]);
        assert_eq!(tl.selected.len(), 4);

        // Nudging moves them all; they can't leave their tracks.
        assert!(tl.nudge(&mut song, &map, 1, Grid::Beat));
        for l in &song.lanes[..2] {
            assert_eq!(l.clips[0].start, 24_000);
        }
        assert_eq!(song.lanes[2].clips[0].start, 0);
        assert!(!tl.shift_lane(&mut song, 1));

        // Trimming one end trims its partner's.
        let second = song.lanes[0].clips[1].id;
        tl.focus = Some((Focus::Clip(second), egui::Id::NULL));
        assert!(tl.trim_focused(&mut song, &map, -1, Grid::Beat, true));
        assert_eq!(song.lanes[0].clips[1].end(), 96_000);
        assert_eq!(song.lanes[1].clips[1].end(), 96_000);

        // With group editing off, a clip is edited alone.
        tl.group_edit = false;
        tl.select_only(&song, second);
        assert_eq!(tl.selected, HashSet::from([second]));
        assert!(tl.trim_focused(&mut song, &map, -1, Grid::Beat, true));
        assert_eq!(song.lanes[0].clips[1].end(), 72_000);
        assert_eq!(song.lanes[1].clips[1].end(), 96_000);
        assert!(tl.shift_lane(&mut song, 1));
    }

    #[test]
    fn crossfading_the_selection_and_zooming_to_it() {
        let mut clips = vec![clip(0, 48_000), clip(46_000, 10_000), clip(90_000, 100)];
        let mut markers = Vec::new();
        let mut tl = Timeline::default();
        let mut song = Song {
            lanes: vec![Lane {
                name: "Guitar",
                clips: &mut clips,
                kind: daw_project::TrackKind::Audio,
                in_folder: false,
                notes: Box::leak(Box::default()),
            }],
            markers: &mut markers,
        };
        assert!(
            tl.crossfade_selected(&mut song, RATE),
            "all, with none selected"
        );
        assert_eq!(song.lanes[0].clips[1].fade_in.len, 2000);
        assert_eq!(song.lanes[0].clips[2].fade_in.len, 0, "too far to meet");
        tl.width = 1000.0;
        tl.selected.insert(song.lanes[0].clips[0].id);
        tl.zoom_to_selection(&song, RATE);
        // A second, with a tenth more, across 1000 points.
        assert!((tl.px_per_sec as f64 - 1000.0 / 1.1).abs() < 0.5);
    }

    #[test]
    fn markers_stay_in_order_and_sections_run_to_the_next() {
        let mut m = Vec::new();
        assert!(add_marker(&mut m, 9));
        assert!(add_marker(&mut m, 1));
        assert!(add_marker(&mut m, 5));
        assert!(!add_marker(&mut m, 5), "one per bar");
        let bars: Vec<i64> = m.iter().map(|m| m.bar).collect();
        assert_eq!(bars, vec![1, 5, 9]);
        assert_eq!(section(&m, 0, 20), (1, 5));
        assert_eq!(section(&m, 2, 20), (9, 20));
        assert_eq!(section(&m, 2, 3), (9, 10), "at least a bar");
    }

    #[test]
    fn splitting_at_the_playhead_takes_the_selection_or_everything() {
        let mut clips = vec![clip(0, 10_000), clip(20_000, 10_000)];
        let mut markers = Vec::new();
        let mut tl = Timeline::default();
        let mut song = Song {
            lanes: vec![Lane {
                name: "Guitar",
                clips: &mut clips,
                kind: daw_project::TrackKind::Audio,
                in_folder: false,
                notes: Box::leak(Box::default()),
            }],
            markers: &mut markers,
        };
        assert!(tl.split_at(&mut song, 5000));
        assert_eq!(song.lanes[0].clips.len(), 3);
        let keys: HashSet<&str> = song.lanes[0].clips.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(keys.len(), 3, "the new half has its own key");
        tl.selected.insert(song.lanes[0].clips[2].id);
        assert!(
            !tl.split_at(&mut song, 2000),
            "only the selected clip, which isn't there"
        );
        assert!(tl.delete_selected(&mut song));
        assert_eq!(song.lanes[0].clips.len(), 2);
        tl.add_marker(&mut song, 3);
        assert_eq!(song.markers.len(), 1);
    }

    #[test]
    fn waveform_ranges_cover_what_is_asked() {
        let base: Vec<(f32, f32)> = (0..64).map(|i| (-(i as f32), i as f32)).collect();
        let w = Waveform::new(base);
        assert_eq!(w.range(0, BASE_HOP), (0.0, 0.0));
        // Coarser levels may reach a little past the range, never short.
        let (lo, hi) = w.range(BASE_HOP * 10, BASE_HOP * 20);
        assert!(
            (-24.0..=-19.0).contains(&lo) && (19.0..=24.0).contains(&hi),
            "{lo} {hi}"
        );
        assert_eq!(w.range(0, BASE_HOP * 64), (-63.0, 63.0));
    }

    #[test]
    fn a_clip_that_follows_tempo_stretches_with_the_map() {
        let map = TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON);
        let mut c = clip(48_000, 48_000);
        c.retime(&map);
        assert!(c.warp(&map).is_none(), "played plainly");
        c.follow_tempo(&map, true);
        assert_eq!(c.tempo, Some(120.0));
        assert_eq!(c.beat, PPQ * 2, "one second in at 120 BPM");
        assert_eq!(c.span, 48_000, "the same length at the song's tempo");
        // Half the tempo: twice as long, starting where its beat now is.
        let slow = TempoMap::new(RATE, Tempo::bpm(60), TimeSignature::COMMON);
        c.start = slow.frames_at(Ticks(c.beat)).0;
        c.retime(&slow);
        assert_eq!(c.start, 96_000);
        assert_eq!(c.span, 96_000);
        assert_eq!(c.end(), 192_000);
        let w = c.warp(&slow).unwrap();
        assert_eq!(w.src_range(), 0..48_000);
        assert_eq!(w.out_range(), 96_000..192_000);
        // Back to its own time, where it is now.
        c.follow_tempo(&slow, false);
        assert_eq!(c.tempo, None);
        assert_eq!(c.span, 48_000);
        assert_eq!(c.start, 96_000);
    }

    #[test]
    fn markers_move_attacks_and_stay_with_their_audio_through_a_split() {
        let map = TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON);
        let mut c = clip(0, 48_000);
        c.retime(&map);
        assert_eq!(c.add_marker(24_000), Some(0));
        assert_eq!(c.add_marker(0), None, "not at the edges");
        assert_eq!(c.add_marker(24_000), Some(0), "the same one");
        assert!(c.move_marker(0, 20_000));
        c.retime(&map);
        assert_eq!(c.out_of_frame(&map, 24_000.0), 20_000);
        assert_eq!(
            c.out_of_frame(&map, 12_000.0),
            10_000,
            "stretched before it"
        );
        // After the last marker the audio plays at the clip's own
        // tempo, so the end comes forward too.
        assert_eq!(c.out_of_frame(&map, 36_000.0), 32_000, "plain after it");
        assert_eq!(c.span, 44_000);
        assert_eq!(c.frame_of_out(&map, 10_000).round() as i64, 12_000);
        let w = c.warp(&map).unwrap();
        assert_eq!(w.points().len(), 3);
        // No more than 3× slower or faster than played.
        assert!(c.move_marker(0, 100_000));
        assert_eq!(c.markers[0].at, 72_000);
        assert!(c.move_marker(0, 1_000));
        assert_eq!(c.markers[0].at, 8_000);
        c.move_marker(0, 20_000);
        // Split after the marker: the first half keeps it, the second
        // is counted from its own start.
        let (a, b) = c.split(30_000).unwrap();
        assert_eq!(a.markers.len(), 1);
        assert_eq!(a.out_of_frame(&map, 24_000.0), 20_000);
        assert!(b.markers.is_empty());
        assert_eq!(b.start, 30_000);
        assert_eq!(a.end(), 30_000);
        assert_eq!(a.span + b.span, 44_000);
        assert_eq!(a.len + b.len, 48_000);
        // Split before it: the second half keeps it where it was.
        let (a, b) = c.split(10_000).unwrap();
        assert!(a.markers.is_empty());
        assert_eq!(b.markers.len(), 1);
        assert_eq!(b.out_of_frame(&map, 24_000.0), 20_000);
        // Trimming the start keeps the marker where it plays.
        let mut t = c.clone();
        t.trim_start(5_000);
        assert_eq!(t.out_of_frame(&map, 24_000.0), 20_000);
        assert_eq!(t.end(), 44_000);
    }

    #[test]
    fn a_tempo_change_inside_a_following_clip_puts_a_warp_point_on_every_beat() {
        let mut map = TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON);
        map.add_tempo(Ticks(PPQ * 4), Tempo::bpm(60)).unwrap();
        let mut c = clip(0, 192_000);
        c.retime(&map);
        c.follow_tempo(&map, true);
        // Eight beats played at 120: four at 120 (2 s), then four at 60
        // (4 s).
        assert_eq!(c.span, 96_000 + 192_000);
        let w = c.warp(&map).unwrap();
        assert_eq!(w.points().len(), 9, "{:?}", w.points());
        assert_eq!(w.out_range(), 0..288_000);
    }
}
