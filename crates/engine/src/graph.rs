// SPDX-License-Identifier: GPL-3.0-or-later
//! The render graph (ARCHITECTURE §4.2): what the audio thread plays.
//!
//! A [`Session`] describes tracks, their clips and mixer settings in
//! musical time. [`Session::compile`] turns it, off the audio thread, into
//! a [`Graph`]: positions converted to frames, gains to linear factors,
//! and every buffer allocated up front. The audio thread only reads it.
//!
//! Each track: audio clips and, when monitoring, the track's input, plus
//! whatever other tracks route into it → the track's insert effects →
//! track (gain, pan, mute, solo, control group) → its output (the master
//! or a bus) and its sends. A bus is a track with no clips of its own
//! (roadmap 2.3). Sends take the audio after the effects and pan, with
//! or without the fader. An effect can listen to another track's audio
//! as a sidechain. Effects can sit in containers that split the audio
//! into several chains and join them again (see [`chain`](crate::chain)).
//!
//! Insert effects ([`Insert`]) are made off the audio thread and live
//! across graphs: a new graph takes them over from the one it replaces
//! by id, so editing the session never restarts a plugin.
//!
//! Delay compensation: a track whose effects add latency has its clips
//! read that far ahead, plus the latency of every bus on its way to the
//! master, so it stays in time with the others. Where a track's audio
//! takes two routes (its output and a send, or a sidechain) that wait
//! different times before the master, the quicker route is delayed to
//! match, so they meet lined up. [`Session::compile_with`] works it all
//! out; `tests/routing.rs` proves it by null tests.

use std::sync::Arc;

use crate::automation::{LaneSpec, Target};
use crate::chain::{self, Step};
use crate::fade::{Fade, GainPoint, Shape};
use crate::layers::{self, Held, LayerNode, LayerSpec};
use crate::metronome::Clicks;
use crate::midi::{Event, Expression, Instrument, Kind, MAX_EVENTS, Outputs};
use crate::monitor::{CueLine, CueSend, MAX_CUES};
use crate::stream::{CHUNK, Chunk, Placement, StreamNode, StreamSource};
use crate::stretch::{Feed, Stretcher};
use crate::time::{Frames, TempoMap, Ticks};

/// Most tracks a graph can hold; meters are sized to this up front.
pub const MAX_TRACKS: usize = 256;

/// One block's level on a track: (left, right) peak and sum of squares.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Level {
    pub peak: (f32, f32),
    pub sq: (f32, f32),
}

/// Decoded audio, one `Vec` per channel (1 or 2 channels).
#[derive(Debug)]
pub struct AudioData {
    pub channels: Vec<Vec<f32>>,
    pub rate: u32,
}

impl AudioData {
    pub fn len(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Where a clip's audio comes from.
#[derive(Clone, Debug)]
pub enum Source {
    /// Decoded and held in memory (short sounds, tests).
    Memory(Arc<AudioData>),
    /// Streamed from a file on disk.
    File(Arc<StreamSource>),
}

impl From<Arc<AudioData>> for Source {
    fn from(d: Arc<AudioData>) -> Source {
        Source::Memory(d)
    }
}

impl From<Arc<StreamSource>> for Source {
    fn from(s: Arc<StreamSource>) -> Source {
        Source::File(s)
    }
}

impl Source {
    fn rate(&self) -> u32 {
        match self {
            Source::Memory(d) => d.rate,
            Source::File(f) => f.rate,
        }
    }

    fn channels(&self) -> usize {
        match self {
            Source::Memory(d) => d.channels.len(),
            Source::File(f) => f.channels,
        }
    }

    fn frames(&self) -> usize {
        match self {
            Source::Memory(d) => d.len(),
            Source::File(f) => f.frames,
        }
    }
}

/// Where a clip starts. Each clip keeps its own time base (decision
/// 0005): musical clips move with the tempo, absolute ones (recorded
/// takes) stay on the frame they were captured at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    Musical(Ticks),
    Absolute(Frames),
}

impl From<Ticks> for Position {
    fn from(t: Ticks) -> Position {
        Position::Musical(t)
    }
}

impl From<Frames> for Position {
    fn from(f: Frames) -> Position {
        Position::Absolute(f)
    }
}

impl Position {
    pub fn frames(self, map: &TempoMap) -> Frames {
        match self {
            Position::Musical(t) => map.frames_at(t),
            Position::Absolute(f) => f,
        }
    }
}

/// A clip placed on a track.
#[derive(Clone, Debug)]
pub struct ClipSpec {
    pub start: Position,
    pub source: Source,
    /// Where in the source playback begins, in frames.
    pub offset: usize,
    /// How much of the source plays, in frames; `None` for the rest of it.
    pub len: Option<usize>,
    pub gain_db: f32,
    /// Fades at the start and end, shortened to fit if together they
    /// are longer than the clip.
    pub fade_in: Fade,
    pub fade_out: Fade,
    /// The clip's gain envelope, in source frames, in order.
    pub envelope: Arc<[GainPoint]>,
}

impl ClipSpec {
    pub fn new(start: impl Into<Position>, source: impl Into<Source>) -> ClipSpec {
        ClipSpec {
            start: start.into(),
            source: source.into(),
            offset: 0,
            len: None,
            gain_db: 0.0,
            fade_in: Fade::default(),
            fade_out: Fade::default(),
            envelope: Arc::new([]),
        }
    }
}

/// An effect in a track's insert chain, run on the audio thread.
pub trait Insert: Send {
    /// Processes one block of stereo audio in place, at most the graph's
    /// `max_block` frames. Must not allocate, lock or block.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]);

    /// As [`process`](Self::process), with a sidechain: another track's
    /// audio for the same frames, already lined up in time, for an effect
    /// that listens to it (a compressor ducking under a voice). An effect
    /// with no use for it processes as usual.
    fn process_keyed(&mut self, left: &mut [f32], right: &mut [f32], key: (&[f32], &[f32])) {
        let _ = key;
        self.process(left, right);
    }

    /// Sets setting `param` for the next block from an automation lane,
    /// instead of the value the effect's own controls hold. Called just
    /// before [`process`](Self::process), once a block, while the lane
    /// plays; a block without a call plays the controls again. An effect
    /// with no settings to set this way ignores it.
    fn set_param(&mut self, param: u32, value: f64) {
        let _ = (param, value);
    }

    /// For an effect container: how it splits the audio among its
    /// chains and joins them again. `None` for an ordinary effect.
    fn as_split(&mut self) -> Option<&mut dyn Split> {
        None
    }
}

/// Most chains an effect container has.
pub const MAX_BRANCHES: usize = 4;

/// How an effect container splits a track's audio into its chains and
/// joins them again. The engine runs each chain's effects on its lane
/// between the two, and lines the chains up in time first. Must not
/// allocate, lock or block, and must add no latency of its own.
pub trait Split: Send {
    /// Fills every lane from the block coming in.
    fn split(&mut self, left: &[f32], right: &[f32], lanes: &mut Lanes);
    /// Writes the block going out from the lanes, after their effects,
    /// and the block that came in (`dry`), delayed to match them.
    fn join(&mut self, dry: (&[f32], &[f32]), lanes: &Lanes, left: &mut [f32], right: &mut [f32]);
}

/// The audio of each of a container's chains for one block: stereo,
/// allocated up front.
pub struct Lanes {
    bufs: Vec<[Vec<f32>; 2]>,
    frames: usize,
}

impl Lanes {
    /// `count` lanes of `frames` frames each, silent.
    pub fn new(count: usize, frames: usize) -> Lanes {
        Lanes {
            bufs: (0..count)
                .map(|_| [vec![0.0; frames], vec![0.0; frames]])
                .collect(),
            frames,
        }
    }

    pub fn count(&self) -> usize {
        self.bufs.len()
    }

    /// Frames in this block.
    pub fn frames(&self) -> usize {
        self.frames
    }

    pub fn get(&self, i: usize) -> (&[f32], &[f32]) {
        let [l, r] = &self.bufs[i];
        (&l[..self.frames], &r[..self.frames])
    }

    pub fn get_mut(&mut self, i: usize) -> (&mut [f32], &mut [f32]) {
        let [l, r] = &mut self.bufs[i];
        (&mut l[..self.frames], &mut r[..self.frames])
    }

    /// Makes the block `frames` long, at most as long as made.
    pub(crate) fn set_frames(&mut self, frames: usize) {
        self.frames = frames.min(self.bufs.first().map_or(frames, |b| b[0].len()));
    }

    pub(crate) fn clear(&mut self) {
        for b in &mut self.bufs {
            b[0].fill(0.0);
            b[1].fill(0.0);
        }
    }
}

/// Where an effect sits when it is inside a container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    /// The container's insert id, on the same track.
    pub container: u64,
    /// Which of its chains, from 0.
    pub branch: u8,
}

/// A slot in a track's insert chain. The effect itself is handed to
/// [`Session::compile_with`] once, when first added, under this `id`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InsertSpec {
    /// Unique for the life of the process.
    pub id: u64,
    pub bypass: bool,
    /// Frames of delay the effect reports, compensated on the track.
    pub latency: u32,
    /// The track whose audio (after its effects and fader) the effect
    /// listens to, lined up in time with this track's.
    pub sidechain: Option<usize>,
    /// For an effect container, how many chains it has (its effect
    /// splits and joins, see [`Split`]); 0 for an ordinary effect. A
    /// container's own `latency` is not counted: its chains' is.
    pub branches: u8,
    /// The container and chain it sits in; `None` for the track's own
    /// chain. The effects in one chain run in the order they are listed.
    pub place: Option<Place>,
}

/// Where a track's audio goes after its fader.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Output {
    #[default]
    Master,
    /// Another track in the session, used as a bus: it has no clips of
    /// its own and sums what is routed to it, through its own effects
    /// and fader.
    Track(usize),
}

/// A copy of a track's audio sent to a bus, as well as its output: for
/// an effect shared by several tracks (a reverb), with each track's
/// amount set here.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SendSpec {
    pub to: usize,
    pub gain_db: f32,
    /// Taken before the track's volume (but after its effects, pan and
    /// mute), so the volume fader leaves the send alone.
    pub pre_fader: bool,
    pub mute: bool,
}

impl SendSpec {
    pub fn to(to: usize) -> SendSpec {
        SendSpec {
            to,
            gain_db: 0.0,
            pre_fader: false,
            mute: false,
        }
    }
}

/// A control group: one fader and mute over several tracks. Its volume
/// adds to each member's, in dB, and its mute silences them all, without
/// their audio ever being summed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GroupSpec {
    pub gain_db: f32,
    pub mute: bool,
}

/// A note placed on the timeline (roadmap 3.1), in ticks, for an
/// instrument track.
/// Where events for a MIDI output go: the port, and the event.
pub type MidiOut<'a> = &'a mut dyn FnMut(usize, &Event);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteSpec {
    pub start: Ticks,
    pub len: Ticks,
    pub pitch: u8,
    pub velocity: u8,
    pub channel: u8,
    /// 0 to 1: the chance it plays, rolled afresh each time the
    /// timeline jumps (a seek, going round the loop, pressing play).
    pub chance: f32,
    /// Notes with the same key share one roll, so they play or don't
    /// together; of `of` (above 1) sharing one, only the one numbered
    /// `pick` plays, chosen at random each time round.
    pub key: u64,
    pub pick: u16,
    pub of: u16,
    /// Its expression: the range of [`TrackSpec::expression`] that is
    /// this note's, empty for most notes.
    pub expr: (u32, u32),
}

/// A point on a note's expression line, on the timeline: from it on,
/// the note's `kind` of expression is `value` (see
/// [`Kind::NoteExpression`]). A note's points are in time order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExprSpec {
    pub at: Ticks,
    pub kind: Expression,
    pub value: f32,
}

/// What a controller lane moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// A MIDI controller by number, values 0 to 127.
    Cc(u8),
    /// The pitch wheel, −8192 to 8191.
    PitchBend,
    /// Pressure on the whole keyboard, 0 to 127.
    Pressure,
}

impl Control {
    fn kind(self, v: i32) -> Kind {
        match self {
            Control::Cc(number) => Kind::Control {
                number: number & 0x7f,
                value: v.clamp(0, 127) as u8,
            },
            Control::PitchBend => Kind::PitchBend(v.clamp(-8192, 8191) as i16),
            Control::Pressure => Kind::Pressure(v.clamp(0, 127) as u8),
        }
    }
}

/// A controller lane on an instrument track (roadmap 3.1): the values
/// a controller takes as the song plays, each from its tick on, in time
/// order. On starting to play, and after a jump, the value in force is
/// sent first, so the instrument is where the lane says.
#[derive(Clone, Debug, PartialEq)]
pub struct ControlSpec {
    pub channel: u8,
    pub control: Control,
    pub points: Arc<[(Ticks, i32)]>,
}

impl NoteSpec {
    /// A note that always plays.
    pub fn new(start: Ticks, len: Ticks, pitch: u8, velocity: u8, channel: u8) -> NoteSpec {
        NoteSpec {
            start,
            len,
            pitch,
            velocity,
            channel,
            chance: 1.0,
            key: 0,
            pick: 0,
            of: 1,
            expr: (0, 0),
        }
    }
}

/// An instrument on a track: the slot the engine hands the block's
/// notes to. The instrument itself is handed to
/// [`Session::compile_with`] once, when first added, under this `id`,
/// like an effect.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InstrumentSpec {
    /// Unique for the life of the process.
    pub id: u64,
    /// Frames of delay the instrument reports, compensated on the track.
    pub latency: u32,
}

/// One of an instrument's extra outputs (see [`Outputs`]) going to a
/// bus: a drum machine's pad on a track of its own, say. It leaves
/// straight from the instrument, before the track's effects and fader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuxSpec {
    /// The output, from 1 (0 is the track itself).
    pub output: u32,
    pub to: usize,
}

/// The most extra outputs an instrument can have routed.
pub const MAX_AUX: u32 = 32;

#[derive(Clone, Debug)]
pub struct TrackSpec {
    pub clips: Vec<ClipSpec>,
    /// Notes the track plays, through its instrument and to its MIDI
    /// output, in any order.
    pub notes: Arc<[NoteSpec]>,
    /// The notes' expression points, each note's run in time order.
    pub expression: Arc<[ExprSpec]>,
    /// Controller lanes, played with the notes.
    pub controls: Vec<ControlSpec>,
    pub instrument: Option<InstrumentSpec>,
    /// Where the instrument's extra outputs go. One route per output;
    /// an output without one is the instrument's to play through the
    /// track or leave out.
    pub instrument_outputs: Vec<AuxSpec>,
    /// A layered instrument (roadmap 3.5): when there are layers, they
    /// play the notes in place of `instrument`.
    pub layers: Vec<LayerSpec>,
    /// Live notes from MIDI devices play through the instrument (and go
    /// to the MIDI output) as they arrive.
    pub notes_live: bool,
    /// A MIDI output port (as the app numbers them) the track's notes
    /// go to, as well as its instrument.
    pub midi_out: Option<usize>,
    pub mix: Mix,
    /// Effects, in processing order.
    pub inserts: Vec<InsertSpec>,
    /// The audio input this track records from and monitors.
    pub input: Option<Input>,
    /// Records its input while the transport records.
    pub arm: bool,
    /// Plays its input through the track (software monitoring).
    pub monitor: bool,
    pub output: Output,
    pub sends: Vec<SendSpec>,
    /// The control group it belongs to, by index in
    /// [`Session::groups`].
    pub group: Option<usize>,
    /// Its level in each headphone mix (see [`crate::monitor`]).
    pub cues: [CueSend; MAX_CUES],
    /// Automation lanes: settings that follow a line as the song plays.
    pub automation: Vec<LaneSpec>,
}

impl TrackSpec {
    pub fn new(clips: Vec<ClipSpec>) -> TrackSpec {
        TrackSpec {
            clips,
            notes: Arc::new([]),
            expression: Arc::new([]),
            controls: Vec::new(),
            instrument: None,
            instrument_outputs: Vec::new(),
            layers: Vec::new(),
            notes_live: false,
            midi_out: None,
            mix: Mix::default(),
            inserts: Vec::new(),
            input: None,
            arm: false,
            monitor: false,
            output: Output::Master,
            sends: Vec::new(),
            group: None,
            cues: [CueSend::default(); MAX_CUES],
            automation: Vec::new(),
        }
    }
}

/// Device input channels (from 0) for a track: one, or a stereo pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Input {
    pub first: usize,
    pub stereo: bool,
}

impl Input {
    pub fn mono(ch: usize) -> Input {
        Input {
            first: ch,
            stereo: false,
        }
    }

    pub fn stereo(first: usize) -> Input {
        Input {
            first,
            stereo: true,
        }
    }

    pub fn channels(self) -> usize {
        if self.stereo { 2 } else { 1 }
    }
}

/// One block of device input, interleaved.
#[derive(Clone, Copy)]
pub(crate) struct InputBlock<'a> {
    pub data: &'a [f32],
    pub channels: usize,
}

impl InputBlock<'_> {
    /// Sample `i` of channel `ch`; silence for a channel the device lacks.
    #[inline]
    pub fn get(&self, i: usize, ch: usize) -> f32 {
        if ch < self.channels {
            self.data
                .get(i * self.channels + ch)
                .copied()
                .unwrap_or(0.0)
        } else {
            0.0
        }
    }
}

/// A track's mixer settings. These can also change while playing,
/// without a new graph (see `Command::SetMix`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mix {
    pub gain_db: f32,
    /// −1 (left) to +1 (right).
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
}

impl Default for Mix {
    fn default() -> Mix {
        Mix {
            gain_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
        }
    }
}

/// Everything the engine plays. Tracks and buses are both
/// [`TrackSpec`]s: a bus is a track with no clips that others route to.
#[derive(Clone, Debug)]
pub struct Session {
    pub tempo: TempoMap,
    pub tracks: Vec<TrackSpec>,
    pub master_db: f32,
    pub groups: Vec<GroupSpec>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CompileError {
    TooManyTracks,
    /// A clip's audio is at a different sample rate from the session.
    /// Converting it is the importer's job.
    RateMismatch {
        track: usize,
    },
    BadChannelCount {
        track: usize,
    },
    /// An output, send, sidechain or control group names a track or
    /// group that isn't there, or the track itself.
    BadRoute {
        track: usize,
    },
    /// Audio would go round in a circle: a bus routed, directly or
    /// through others, back into a track that feeds it.
    Cycle {
        track: usize,
    },
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::TooManyTracks => write!(f, "more than {MAX_TRACKS} tracks"),
            CompileError::RateMismatch { track } => {
                write!(
                    f,
                    "track {}: clip audio is at another sample rate",
                    track + 1
                )
            }
            CompileError::BadChannelCount { track } => {
                write!(f, "track {}: clip audio must be mono or stereo", track + 1)
            }
            CompileError::BadRoute { track } => {
                write!(f, "track {}: routed to a track that isn't there", track + 1)
            }
            CompileError::Cycle { track } => {
                write!(
                    f,
                    "track {}: its audio would come back round to itself",
                    track + 1
                )
            }
        }
    }
}

impl std::error::Error for CompileError {}

pub fn db_to_gain(db: f32) -> f32 {
    if db <= -96.0 {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

/// Left and right gains for a track.
///
/// Mono sources are panned with a constant-power law (−3 dB each side in
/// the centre). Stereo sources use a balance control: the centre leaves
/// both channels untouched, and turning towards one side fades the other.
pub fn pan_gains(pan: f32, stereo: bool) -> (f32, f32) {
    let p = pan.clamp(-1.0, 1.0);
    if stereo {
        ((1.0 - p).min(1.0), (1.0 + p).min(1.0))
    } else {
        let a = (p + 1.0) * std::f32::consts::FRAC_PI_4;
        (a.cos(), a.sin())
    }
}

pub(crate) struct ClipNode {
    pub start: i64,
    pub end: i64,
    pub offset: usize,
    pub shape: Shape,
    pub src: NodeSrc,
}

pub(crate) enum NodeSrc {
    Memory(Arc<AudioData>),
    /// Index into `Graph::streams`.
    Stream(usize),
}

/// A note as the audio thread plays it: frames on the timeline.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NoteNode {
    pub start: i64,
    pub end: i64,
    pub pitch: u8,
    pub velocity: u8,
    pub channel: u8,
    pub chance: f32,
    pub key: u64,
    pub pick: u16,
    pub of: u16,
    /// Its expression points, as a range of the track's `exprs`.
    pub expr: (usize, usize),
}

/// A note's expression point, in timeline frames.
pub(crate) struct ExprNode {
    pub at: i64,
    pub kind: Expression,
    pub value: f32,
}

/// A controller lane, in timeline frames.
pub(crate) struct ControlNode {
    pub channel: u8,
    pub control: Control,
    pub points: Vec<(i64, i32)>,
}

impl NoteNode {
    /// Whether it plays this time round: `pass` counts the timeline's
    /// jumps, so each time round rolls afresh, the same way for every
    /// note sharing its key.
    fn plays(&self, pass: u64) -> bool {
        if self.of > 1 && mix(self.key, pass) % self.of as u64 != self.pick as u64 {
            return false;
        }
        if self.chance >= 1.0 {
            return true;
        }
        let u = (mix(self.key ^ 0x5eed, pass) >> 11) as f64 / (1u64 << 53) as f64;
        u < self.chance as f64
    }
}

/// SplitMix64 of `a` and `b`.
fn mix(a: u64, b: u64) -> u64 {
    let mut z = a ^ b.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

pub(crate) struct InstrumentNode {
    pub id: u64,
    /// Taken over from the previous graph, or handed in when new.
    pub inst: Option<Box<dyn Instrument>>,
}

/// The most notes one track can have sounding at once from its clips;
/// past it, extra notes start but are not stopped until the next
/// all-notes-off.
const MAX_SOUNDING: usize = 256;

pub(crate) struct TrackNode {
    /// Sorted by start.
    pub clips: Vec<ClipNode>,
    /// Sorted by start.
    pub notes: Vec<NoteNode>,
    pub exprs: Vec<ExprNode>,
    pub controls: Vec<ControlNode>,
    pub instrument: Option<InstrumentNode>,
    pub layers: Vec<LayerNode>,
    pub notes_live: bool,
    pub midi_out: Option<usize>,
    /// This block's events, in frame order; capacity [`MAX_EVENTS`].
    pub events: Vec<Event>,
    /// Notes from clips sounding now, by index into `notes`; capacity
    /// [`MAX_SOUNDING`].
    pub sounding: Vec<usize>,
    /// Whether the last block played; stopping sends every note off.
    pub was_playing: bool,
    /// Where the last block's notes ended on the timeline, so a jump
    /// (a seek, the loop) stops the notes still sounding.
    pub notes_at: Option<i64>,
    /// How many times the timeline has jumped, for rolling notes'
    /// chance afresh each time round.
    pub pass: u64,
    pub mix: Mix,
    /// Gains reached at the end of the last block, for smoothing.
    pub last: (f32, f32),
    pub stereo: bool,
    pub input: Option<Input>,
    pub arm: bool,
    pub monitor: bool,
    pub inserts: Vec<InsertNode>,
    /// The order they run in, with containers' chains.
    pub chain: Vec<Step>,
    /// Practice speed and transpose; used only when either is set.
    pub stretch: Box<Stretcher>,
    /// How far ahead of the play position the clips are read: the
    /// effects' own latency, plus that of every bus on the way to the
    /// master (see [`Graph::render`] on delay compensation).
    pub latency: i64,
    /// The source is mono but the effects are stereo: copy left to right
    /// before them.
    pub widen: bool,
    pub group: Option<usize>,
    /// Edges into this track (outputs and sends routed here, and the
    /// sidechains its effects listen to), as indices into `Graph::edges`.
    pub incoming: Vec<usize>,
    /// Soloing: this track is heard because it, or a track routed into
    /// it, is soloed.
    pub heard: bool,
    /// Its levels in the headphone mixes, the gains reached at the end
    /// of the last block, and the delay lining them up with the master.
    pub cues: [CueSend; MAX_CUES],
    pub cue_last: [(f32, f32); MAX_CUES],
    pub cue_line: CueLine,
    pub automation: Vec<LaneSpec>,
    /// The volume and pan the lanes ask for this block, if they do.
    pub auto_gain: Option<f32>,
    pub auto_pan: Option<f32>,
    /// Edges out of this track (its output and sends), as indices into
    /// `Graph::edges`.
    pub outgoing: Vec<usize>,
}

pub(crate) struct InsertNode {
    pub id: u64,
    pub bypass: bool,
    /// Taken over from the previous graph, or handed in when new.
    pub effect: Option<Box<dyn Insert>>,
    /// The edge that brings its sidechain, if it has one.
    pub key: Option<usize>,
}

/// What an edge carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Carry {
    /// The track's output, after its fader.
    Output,
    /// A send: the index into the track's sends.
    Send(usize),
    /// A sidechain into the effect at this index of the target's chain.
    Sidechain(usize),
    /// An extra output of the track's instrument, from 1.
    Aux(u32),
}

/// Audio going from one track to another (or to the master), delayed so
/// that every path from a sound to the master takes the same time.
pub(crate) struct Edge {
    pub from: usize,
    /// `None` for the master.
    pub to: Option<usize>,
    pub carry: Carry,
    /// Taken before the fader (sends only).
    pub pre_fader: bool,
    /// The gain asked for (a send's level, or 0 when muted), and the
    /// gain reached at the end of the last block.
    pub gain: f32,
    pub last: f32,
    /// Muted (sends only): a lane on the send's level leaves it silent.
    pub mute: bool,
    /// A ring buffer of `delay` frames per channel, or empty for none.
    pub line: [Vec<f32>; 2],
    pub at: usize,
}

impl Edge {
    fn delay(&self) -> usize {
        self.line[0].len()
    }

    /// Runs `src` through the delay line, scaled by the gain ramping
    /// from `last` to `gain`, adding the result into `dst`.
    pub(crate) fn feed(&mut self, src: (&[f32], &[f32]), dst: (&mut [f32], &mut [f32])) {
        let n = src.0.len();
        let d = self.delay();
        let step = 1.0 / n.max(1) as f32;
        let (g0, g1) = (self.last, self.gain);
        for i in 0..n {
            let g = g0 + (g1 - g0) * (i + 1) as f32 * step;
            let (mut a, mut b) = (src.0[i], src.1[i]);
            if d > 0 {
                a = std::mem::replace(&mut self.line[0][self.at], a);
                b = std::mem::replace(&mut self.line[1][self.at], b);
                self.at = (self.at + 1) % d;
            }
            dst.0[i] += a * g;
            dst.1[i] += b * g;
        }
        self.last = g1;
    }
}

impl ClipNode {
    /// The level of the clip's frame `rel`.
    #[inline]
    fn gain(&self, rel: usize) -> f32 {
        self.shape
            .at(rel, (self.end - self.start) as usize, self.offset)
    }
}

impl TrackNode {
    /// Left and right gains the mixer settings ask for, with the fader
    /// (`volume`) or without it (what a pre-fader send takes).
    fn target(&self, any_solo: bool, groups: &[GroupSpec], volume: bool) -> (f32, f32) {
        let group = self.group.and_then(|g| groups.get(g));
        if self.mix.mute || group.is_some_and(|g| g.mute) || any_solo && !self.heard {
            return (0.0, 0.0);
        }
        let g = if volume {
            let own = self.auto_gain.unwrap_or(self.mix.gain_db);
            db_to_gain(own + group.map_or(0.0, |g| g.gain_db))
        } else {
            1.0
        };
        let (l, r) = pan_gains(self.auto_pan.unwrap_or(self.mix.pan), self.stereo);
        (g * l, g * r)
    }

    /// Reads the lanes at `at` into the volume, pan and the effects, and
    /// gives each send's level, to set on its edge.
    fn read_lanes(&mut self, at: i64, sends: &mut [(usize, f32)], count: &mut usize) {
        self.auto_gain = None;
        self.auto_pan = None;
        *count = 0;
        for lane in &self.automation {
            if lane.live {
                continue;
            }
            let Some(v) = lane.curve.at(at) else { continue };
            match lane.target {
                Target::Gain => self.auto_gain = Some(v),
                Target::Pan => self.auto_pan = Some(v.clamp(-1.0, 1.0)),
                Target::Send(k) => {
                    if *count < sends.len() {
                        sends[*count] = (k, v);
                        *count += 1;
                    }
                }
                Target::Insert { id, param } => {
                    if let Some(i) = self.inserts.iter_mut().find(|i| i.id == id)
                        && let Some(fx) = &mut i.effect
                    {
                        fx.set_param(param, v as f64);
                    }
                }
            }
        }
    }
}

/// The most send lanes one track reads a block.
const MAX_SEND_LANES: usize = 16;

/// Adds the clips' audio for frames `[pos, pos + l.len())` into `l` and
/// `r`. Returns how many frames of streamed audio were missing.
fn read_clips(
    clips: &[ClipNode],
    streams: &[StreamNode],
    pos: i64,
    sl: &mut [f32],
    sr: &mut [f32],
) -> usize {
    let n = sl.len();
    let mut missing = 0;
    // Clips are sorted by start; skip those that end before this
    // block, stop at the first that starts after it.
    for c in clips {
        if c.start >= pos + n as i64 {
            break;
        }
        if c.end <= pos {
            continue;
        }
        let from = c.start.max(pos);
        let to = c.end.min(pos + n as i64);
        let dst = (from - pos) as usize;
        let rel = (from - c.start) as usize;
        let src = c.offset + rel;
        let len = (to - from) as usize;
        match &c.src {
            NodeSrc::Memory(d) => {
                let chans = &d.channels;
                let right = chans.get(1).unwrap_or(&chans[0]);
                let flat = c.shape.is_flat();
                for i in 0..len {
                    let g = if flat { c.shape.gain } else { c.gain(rel + i) };
                    sl[dst + i] += chans[0][src + i] * g;
                    sr[dst + i] += right[src + i] * g;
                }
            }
            NodeSrc::Stream(si) => {
                let slots = &streams[*si].slots;
                let mut done = 0;
                // One chunk at a time.
                while done < len {
                    let f = src + done;
                    let (ci, within) = (f / CHUNK, f % CHUNK);
                    let run = (len - done).min(CHUNK - within);
                    match slots.get(ci).and_then(|s| s.as_deref()) {
                        Some(ch) => {
                            let run = run.min(ch.frames().saturating_sub(within));
                            mix_chunk(
                                ch,
                                within,
                                run,
                                c,
                                rel + done,
                                &mut sl[dst + done..],
                                &mut sr[dst + done..],
                            );
                            missing += (len - done).min(CHUNK - within) - run;
                        }
                        None => missing += run,
                    }
                    done += (len - done).min(CHUNK - within);
                }
            }
        }
    }
    missing
}

/// Adds the clips' audio for `l.len()` frames from `start` into `l` and
/// `r`, where `start` is in unwrapped play time: past the loop end it
/// goes back round the loop. Returns how many frames of streamed audio
/// were missing.
fn read_looped(
    clips: &[ClipNode],
    streams: &[StreamNode],
    looping: Option<(i64, i64)>,
    start: i64,
    l: &mut [f32],
    r: &mut [f32],
) -> usize {
    let mut missing = 0;
    let mut done = 0;
    while done < l.len() {
        let u = start + done as i64;
        // Where `u` falls on the timeline, and how far until the loop
        // sends it back.
        let (pos, run) = match looping {
            Some((a, b)) if u >= b && b > a => {
                let p = a + (u - b).rem_euclid(b - a);
                (p, (b - p) as usize)
            }
            Some((_, b)) if u < b => (u, (b - u) as usize),
            _ => (u, usize::MAX),
        };
        let run = run.min(l.len() - done);
        let (sl, sr) = (&mut l[done..done + run], &mut r[done..done + run]);
        missing += read_clips(clips, streams, pos, sl, sr);
        done += run;
    }
    missing
}

/// A track's clips as the stretcher reads them: by position in
/// unwrapped play time, which goes round the loop.
struct TrackFeed<'a> {
    clips: &'a [ClipNode],
    streams: &'a [StreamNode],
    looping: Option<(i64, i64)>,
    missing: usize,
}

impl Feed for TrackFeed<'_> {
    fn read(&mut self, start: i64, l: &mut [f32], r: &mut [f32]) {
        l.fill(0.0);
        r.fill(0.0);
        self.missing += read_looped(self.clips, self.streams, self.looping, start, l, r);
    }
}

/// Practice speed and transpose for one block of [`Graph::render`].
pub(crate) struct Rate {
    /// Play position at the block's first frame, in unwrapped play time:
    /// it runs on past the loop end, which the feed maps back.
    pub u: f64,
    pub speed: f64,
    pub semitones: f64,
    /// Start the stretchers afresh at `u` (play started, seek, loop
    /// changed).
    pub reset: bool,
}

/// Adds `run` frames of a chunk, from frame `at`, into the two buffers,
/// at clip `c`'s level from its frame `rel` on.
#[allow(clippy::too_many_arguments)]
fn mix_chunk(
    ch: &Chunk,
    at: usize,
    run: usize,
    c: &ClipNode,
    rel: usize,
    l: &mut [f32],
    r: &mut [f32],
) {
    let k = ch.channels;
    let s = &ch.samples[at * k..(at + run) * k];
    if c.shape.is_flat() {
        let gain = c.shape.gain;
        for (i, frame) in s.chunks_exact(k).enumerate() {
            l[i] += frame[0] * gain;
            r[i] += frame[k - 1] * gain;
        }
    } else {
        for (i, frame) in s.chunks_exact(k).enumerate() {
            let gain = c.gain(rel + i);
            l[i] += frame[0] * gain;
            r[i] += frame[k - 1] * gain;
        }
    }
}

/// Works out the note events for `n` output frames from `from` on the
/// timeline (in unwrapped play time, which `looping` sends back round
/// the loop), with each output frame covering `speed` timeline frames:
/// notes starting in the block go on, sounding notes ending in it go
/// off. A jump in the timeline (a seek, or going round the loop) stops
/// every sounding note first, and counts one more `pass`, for chance.
/// Events are added to `events` in frame order, up to its capacity.
#[allow(clippy::too_many_arguments)]
fn schedule_notes(
    notes: &[NoteNode],
    exprs: &[ExprNode],
    controls: &[ControlNode],
    sounding: &mut Vec<usize>,
    at: &mut Option<i64>,
    pass: &mut u64,
    looping: Option<(i64, i64)>,
    from: f64,
    speed: f64,
    n: usize,
    events: &mut Vec<Event>,
) {
    if notes.is_empty() && sounding.is_empty() && controls.is_empty() {
        return;
    }
    let speed = if speed > 0.0 { speed } else { 1.0 };
    let mut push = |e: Event| {
        if events.len() < events.capacity() {
            events.push(e);
        }
    };
    let stop_all = |sounding: &mut Vec<usize>, frame: u32, push: &mut dyn FnMut(Event)| {
        for &i in sounding.iter() {
            let nn = &notes[i];
            push(Event::note_off(frame, nn.channel, nn.pitch, i as u32));
        }
        sounding.clear();
    };
    // The block in timeline frames, split where the loop sends it back.
    let mut done = 0.0f64;
    let total = n as f64 * speed;
    while done < total {
        let u = from + done;
        let (pos, run) = match looping {
            Some((a, b)) if u >= b as f64 && b > a => {
                let len = (b - a) as f64;
                let p = a as f64 + (u - b as f64).rem_euclid(len);
                (p, b as f64 - p)
            }
            Some((_, b)) if u < b as f64 => (u, b as f64 - u),
            _ => (u, f64::INFINITY),
        };
        let run = run.min(total - done).max(1e-9);
        // Timeline frames [start, end) this stretch covers, following
        // on from the last block where it does.
        let start = match *at {
            Some(a) if (a as f64 - pos).abs() <= 1.0 => a,
            _ => pos.floor() as i64,
        };
        let end = ((pos + run).ceil() as i64).max(start + 1);
        let frame_of = |t: i64| -> u32 {
            (((t as f64 - pos) / speed + done / speed).floor().max(0.0) as u32).min(n as u32 - 1)
        };
        if at.is_some_and(|a| a != start) {
            stop_all(sounding, frame_of(start), &mut push);
        }
        // Controllers: after a jump, or on starting, the value in force
        // here; then the points in the stretch.
        let jumped = at.is_none_or(|a| a != start);
        for c in controls {
            let first = c.points.partition_point(|p| p.0 < start);
            if jumped && first > 0 && c.points.get(first).is_none_or(|p| p.0 != start) {
                let v = c.points[first - 1].1;
                push(Event {
                    frame: frame_of(start),
                    channel: c.channel,
                    kind: c.control.kind(v),
                });
            }
            for &(t, v) in c.points[first..].iter().take_while(|p| p.0 < end) {
                push(Event {
                    frame: frame_of(t),
                    channel: c.channel,
                    kind: c.control.kind(v),
                });
            }
        }
        // A note's expression points in [from, to).
        let express = |i: usize, from: i64, to: i64, push: &mut dyn FnMut(Event)| {
            let nn = &notes[i];
            let points = &exprs[nn.expr.0..nn.expr.1];
            let first = points.partition_point(|p| p.at < from);
            for p in points[first..].iter().take_while(|p| p.at < to) {
                push(Event {
                    frame: frame_of(p.at),
                    channel: nn.channel,
                    kind: Kind::NoteExpression {
                        id: i as u32,
                        pitch: nn.pitch,
                        kind: p.kind,
                        value: p.value,
                    },
                });
            }
        };
        if at.is_none_or(|a| a != start) {
            *pass = pass.wrapping_add(1);
        }
        // Notes ending in this stretch.
        let mut k = 0;
        while k < sounding.len() {
            let i = sounding[k];
            let nn = &notes[i];
            express(i, start, end.min(nn.end), &mut push);
            if nn.end < end {
                push(Event::note_off(
                    frame_of(nn.end.max(start)),
                    nn.channel,
                    nn.pitch,
                    i as u32,
                ));
                sounding.swap_remove(k);
            } else {
                k += 1;
            }
        }
        // Notes starting in it: binary search for the first.
        let first = notes.partition_point(|nn| nn.start < start);
        for (i, nn) in notes.iter().enumerate().skip(first) {
            if nn.start >= end {
                break;
            }
            if !nn.plays(*pass) {
                continue;
            }
            push(Event::note_on(
                frame_of(nn.start),
                nn.channel,
                nn.pitch,
                nn.velocity,
                i as u32,
            ));
            express(i, nn.start, end.min(nn.end), &mut push);
            if nn.end < end {
                push(Event::note_off(
                    frame_of(nn.end),
                    nn.channel,
                    nn.pitch,
                    i as u32,
                ));
            } else if sounding.len() < sounding.capacity() {
                sounding.push(i);
            }
        }
        *at = Some(end);
        done += run;
    }
}

/// The compiled, immutable-shape graph the audio thread plays.
pub struct Graph {
    pub(crate) tempo: TempoMap,
    pub(crate) tracks: Vec<TrackNode>,
    pub(crate) master: f32,
    pub(crate) any_solo: bool,
    pub(crate) groups: Vec<GroupSpec>,
    /// Per-track scratch, two channels each, `max_block` long: the
    /// track's sum before its effects, then after them.
    pub(crate) scratch: Vec<[Vec<f32>; 2]>,
    /// Per-track audio after the effects and pan, without the fader
    /// (for pre-fader sends), and after the fader too.
    pub(crate) pre: Vec<[Vec<f32>; 2]>,
    pub(crate) post: Vec<[Vec<f32>; 2]>,
    /// One sidechain block, filled just before the effect that listens
    /// to it runs.
    key: [Vec<f32>; 2],
    /// Per track, its instrument's extra outputs (see [`Outputs`]) this
    /// block, and which of them go somewhere.
    aux: Vec<Vec<[Vec<f32>; 2]>>,
    aux_routed: Vec<Vec<bool>>,
    pub(crate) edges: Vec<Edge>,
    /// Edges into the master.
    to_master: Vec<usize>,
    /// Tracks in the order they must be rendered: every track before
    /// the ones it feeds.
    order: Vec<usize>,
    pub(crate) clicks: Clicks,
    pub(crate) streams: Vec<StreamNode>,
    /// The file each stream reads, for the disk thread.
    placements: Vec<Placement>,
    max_block: usize,
}

impl Session {
    pub fn new(tempo: TempoMap) -> Session {
        Session {
            tempo,
            tracks: Vec::new(),
            master_db: 0.0,
            groups: Vec::new(),
        }
    }

    /// Checks every route, and finds an order to render the tracks in
    /// with each before the tracks it feeds. Fails on a route to a track
    /// that isn't there, or one that goes round in a circle.
    fn route_order(&self) -> Result<Vec<usize>, CompileError> {
        let n = self.tracks.len();
        let mut indegree = vec![0usize; n];
        // Edges as (from, to) for the order.
        let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, t) in self.tracks.iter().enumerate() {
            let bad = CompileError::BadRoute { track: i };
            let mut to = Vec::new();
            if let Output::Track(j) = t.output {
                to.push(j);
            }
            to.extend(t.sends.iter().map(|s| s.to));
            for a in &t.instrument_outputs {
                if a.output == 0 || a.output > MAX_AUX {
                    return Err(bad);
                }
                to.push(a.to);
            }
            for j in to {
                if j >= n || j == i {
                    return Err(bad);
                }
                out[i].push(j);
                indegree[j] += 1;
            }
            // A sidechain comes the other way: its track feeds this one.
            for j in t.inserts.iter().filter_map(|i| i.sidechain) {
                if j >= n || j == i {
                    return Err(bad);
                }
                out[j].push(i);
                indegree[i] += 1;
            }
            if t.group.is_some_and(|g| g >= self.groups.len()) {
                return Err(bad);
            }
        }
        // Kahn's algorithm: tracks with nothing feeding them come first.
        let mut ready: Vec<usize> = (0..n).filter(|&i| indegree[i] == 0).collect();
        ready.reverse();
        let mut order = Vec::with_capacity(n);
        while let Some(i) = ready.pop() {
            order.push(i);
            for &j in &out[i] {
                indegree[j] -= 1;
                if indegree[j] == 0 {
                    ready.push(j);
                }
            }
        }
        if order.len() < n {
            let track = (0..n).find(|&i| indegree[i] > 0).unwrap_or(0);
            return Err(CompileError::Cycle { track });
        }
        Ok(order)
    }

    /// Builds the graph. Allocates; never call on the audio thread.
    pub fn compile(&self, max_block: usize) -> Result<Graph, CompileError> {
        self.compile_with(max_block, &mut Vec::new())
    }

    /// As [`compile`](Self::compile), taking newly made effects from
    /// `new_effects` by insert id. Effects already playing are taken over
    /// from the current graph when this one replaces it.
    pub fn compile_with(
        &self,
        max_block: usize,
        new_effects: &mut Vec<(u64, Box<dyn Insert>)>,
    ) -> Result<Graph, CompileError> {
        self.compile_all(max_block, new_effects, &mut Vec::new())
    }

    /// As [`compile_with`](Self::compile_with), also taking newly made
    /// instruments from `new_instruments` by id.
    pub fn compile_all(
        &self,
        max_block: usize,
        new_effects: &mut Vec<(u64, Box<dyn Insert>)>,
        new_instruments: &mut Vec<(u64, Box<dyn Instrument>)>,
    ) -> Result<Graph, CompileError> {
        if self.tracks.len() > MAX_TRACKS {
            return Err(CompileError::TooManyTracks);
        }
        let order = self.route_order()?;
        let rate = self.tempo.sample_rate();
        let mut tracks = Vec::with_capacity(self.tracks.len());
        let mut streams: Vec<StreamNode> = Vec::new();
        let mut placements = Vec::new();
        // For each track, how long after its input each effect runs.
        let mut befores: Vec<Vec<i64>> = Vec::with_capacity(self.tracks.len());
        // Each track's effects' latency, without its instrument's.
        let mut chain_latency: Vec<i64> = Vec::with_capacity(self.tracks.len());
        let mut aux: Vec<Vec<[Vec<f32>; 2]>> = Vec::with_capacity(self.tracks.len());
        let mut aux_routed: Vec<Vec<bool>> = Vec::with_capacity(self.tracks.len());
        for (ti, t) in self.tracks.iter().enumerate() {
            let mut clips = Vec::with_capacity(t.clips.len());
            let mut stereo = false;
            for c in &t.clips {
                if c.source.rate() != rate {
                    return Err(CompileError::RateMismatch { track: ti });
                }
                match c.source.channels() {
                    1 => {}
                    2 => stereo = true,
                    _ => return Err(CompileError::BadChannelCount { track: ti }),
                }
                let avail = c.source.frames().saturating_sub(c.offset);
                let len = c.len.unwrap_or(avail).min(avail);
                if len == 0 {
                    continue;
                }
                let start = c.start.frames(&self.tempo).0;
                let src = match &c.source {
                    Source::Memory(d) => NodeSrc::Memory(d.clone()),
                    Source::File(f) => {
                        placements.push(Placement {
                            source: f.clone(),
                            start,
                            end: start + len as i64,
                            offset: c.offset,
                        });
                        let i = match streams.iter().position(|s| s.id == f.id) {
                            Some(i) => i,
                            None => {
                                streams.push(StreamNode {
                                    id: f.id,
                                    slots: (0..f.chunks()).map(|_| None).collect(),
                                });
                                streams.len() - 1
                            }
                        };
                        NodeSrc::Stream(i)
                    }
                };
                clips.push(ClipNode {
                    start,
                    end: start + len as i64,
                    offset: c.offset,
                    shape: Shape {
                        fade_in: c.fade_in,
                        fade_out: c.fade_out,
                        envelope: c.envelope.clone(),
                        ..Shape::flat(c.gain_db)
                    }
                    .fitted(len),
                    src,
                });
            }
            clips.sort_by_key(|c| c.start);
            let mut exprs: Vec<ExprNode> = Vec::new();
            let mut notes: Vec<NoteNode> = (t.notes.iter())
                .filter(|n| n.len.0 > 0 && n.pitch <= 127)
                .map(|n| {
                    let start = self.tempo.frames_at(n.start).0;
                    let end = self.tempo.frames_at(Ticks(n.start.0 + n.len.0)).0;
                    let from = exprs.len();
                    let (a, b) = (n.expr.0 as usize, n.expr.1 as usize);
                    if let Some(points) = t.expression.get(a..b) {
                        exprs.extend(points.iter().map(|p| ExprNode {
                            at: self.tempo.frames_at(p.at).0,
                            kind: p.kind,
                            value: p.value,
                        }));
                        exprs[from..].sort_by_key(|p| p.at);
                    }
                    NoteNode {
                        start,
                        end: end.max(start + 1),
                        pitch: n.pitch,
                        velocity: n.velocity.clamp(1, 127),
                        channel: n.channel & 15,
                        expr: (from, exprs.len()),
                        chance: n.chance.clamp(0.0, 1.0),
                        key: n.key,
                        pick: n.pick,
                        of: n.of.max(1),
                    }
                })
                .collect();
            notes.sort_by_key(|n| (n.start, n.pitch));
            let controls: Vec<ControlNode> = (t.controls.iter())
                .filter(|c| !c.points.is_empty())
                .map(|c| {
                    let mut points: Vec<(i64, i32)> = (c.points.iter())
                        .map(|&(at, v)| (self.tempo.frames_at(at).0, v))
                        .collect();
                    points.sort_by_key(|p| p.0);
                    ControlNode {
                        channel: c.channel & 15,
                        control: c.control,
                        points,
                    }
                })
                .collect();
            let layered = !t.layers.is_empty();
            let instrument = t.instrument.filter(|_| !layered).map(|i| InstrumentNode {
                id: i.id,
                inst: new_instruments
                    .iter()
                    .position(|(id, _)| *id == i.id)
                    .map(|k| new_instruments.swap_remove(k).1),
            });
            let monitor = t.monitor && t.input.is_some();
            if monitor && t.input.is_some_and(|i| i.stereo) {
                stereo = true;
            }
            let inserts: Vec<InsertNode> = t
                .inserts
                .iter()
                .map(|i| InsertNode {
                    id: i.id,
                    bypass: i.bypass,
                    effect: new_effects
                        .iter()
                        .position(|(id, _)| *id == i.id)
                        .map(|k| new_effects.swap_remove(k).1),
                    key: None,
                })
                .collect();
            let lay: Vec<(u64, i64)> = (t.layers.iter())
                .map(|l| (l.id, l.instrument.map_or(0, |i| i.latency as i64)))
                .collect();
            let built = chain::build(&t.inserts, &lay, max_block);
            // A layered instrument's latency is in the chain's.
            let latency = built.latency
                + instrument
                    .as_ref()
                    .and(t.instrument)
                    .map_or(0, |i| i.latency as i64);
            befores.push(built.before);
            // The extra outputs skip the track's chain.
            chain_latency.push(built.latency);
            // Buffers for the instrument's routed extra outputs, up to
            // the highest.
            let routes = if instrument.is_some() {
                &t.instrument_outputs[..]
            } else {
                &[]
            };
            let count = routes.iter().map(|a| a.output as usize).max().unwrap_or(0);
            aux.push(
                (0..count)
                    .map(|_| [vec![0.0; max_block], vec![0.0; max_block]])
                    .collect(),
            );
            aux_routed.push(
                (1..=count)
                    .map(|k| routes.iter().any(|a| a.output as usize == k))
                    .collect(),
            );
            let layers: Vec<LayerNode> = (t.layers.iter())
                .zip(built.layers)
                .map(|(l, c)| LayerNode::new(l, c, new_instruments, max_block))
                .collect();
            // A bus sums stereo audio.
            let fed = self.tracks.iter().any(|o| {
                o.output == Output::Track(ti)
                    || o.sends.iter().any(|s| s.to == ti)
                    || o.instrument.is_some() && o.instrument_outputs.iter().any(|a| a.to == ti)
            });
            stereo |= fed;
            // Effects and instruments work in stereo, so a mono track
            // becomes stereo.
            let plays = instrument.is_some() || !layers.is_empty();
            let widen = !stereo && !inserts.is_empty() && !plays;
            stereo |= !inserts.is_empty() || plays;
            tracks.push(TrackNode {
                clips,
                notes,
                exprs,
                controls,
                instrument,
                layers,
                notes_live: t.notes_live,
                midi_out: t.midi_out,
                events: Vec::with_capacity(MAX_EVENTS),
                sounding: Vec::with_capacity(MAX_SOUNDING),
                was_playing: false,
                notes_at: None,
                pass: 0,
                mix: t.mix,
                last: (0.0, 0.0),
                stereo,
                input: t.input,
                arm: t.arm && t.input.is_some(),
                monitor,
                inserts,
                chain: built.steps,
                latency,
                widen,
                stretch: Box::new(Stretcher::new(max_block)),
                group: t.group,
                incoming: Vec::new(),
                heard: false,
                cues: t.cues,
                cue_last: [(0.0, 0.0); MAX_CUES],
                cue_line: CueLine::default(),
                automation: t.automation.clone(),
                auto_gain: None,
                auto_pan: None,
                outgoing: Vec::new(),
            });
        }

        // Delay compensation. Each track's audio must reach the master
        // at the same time whichever way it goes. `downstream[i]` is the
        // longest wait from track `i`'s fader to the master, over every
        // route it takes (its output, sends and sidechains): the tracks
        // on the way each add their effects' latency. Track `i` reads
        // its clips ahead by its own effects' latency plus `downstream`,
        // and each route that waits less than `downstream` is delayed by
        // the difference, so they all line up again where they meet.
        let n = tracks.len();
        let own: Vec<i64> = tracks.iter().map(|t| t.latency).collect();
        let mut downstream = vec![0i64; n];
        let mut edges: Vec<Edge> = Vec::new();
        let mut to_master = Vec::new();
        // How long after the target's input its effect `k` runs: the
        // latency of the effects before it.
        let before = |j: usize, k: usize| -> i64 { befores[j][k] };
        for &i in order.iter().rev() {
            let t = &self.tracks[i];
            // (to, carry, pre_fader, gain, the wait on that route)
            let mut routes: Vec<(Option<usize>, Carry, bool, f32, i64)> = Vec::new();
            let wait = |j: usize| downstream[j] + own[j];
            match t.output {
                Output::Master => routes.push((None, Carry::Output, false, 1.0, 0)),
                Output::Track(j) => routes.push((Some(j), Carry::Output, false, 1.0, wait(j))),
            }
            for (k, s) in t.sends.iter().enumerate() {
                let gain = if s.mute { 0.0 } else { db_to_gain(s.gain_db) };
                routes.push((Some(s.to), Carry::Send(k), s.pre_fader, gain, wait(s.to)));
            }
            // An extra output leaves before the track's effects, so it
            // waits that much less than the fader's audio.
            if t.instrument.is_some() {
                let mut seen = Vec::new();
                for a in &t.instrument_outputs {
                    if seen.contains(&a.output) {
                        continue;
                    }
                    seen.push(a.output);
                    let w = wait(a.to) - chain_latency[i];
                    routes.push((Some(a.to), Carry::Aux(a.output), false, 1.0, w));
                }
            }
            for (j, o) in self.tracks.iter().enumerate() {
                for (k, ins) in o.inserts.iter().enumerate() {
                    if ins.sidechain == Some(i) {
                        let w = wait(j) - before(j, k);
                        routes.push((Some(j), Carry::Sidechain(k), false, 1.0, w));
                    }
                }
            }
            let longest = routes.iter().map(|r| r.4).max().unwrap_or(0).max(0);
            downstream[i] = longest;
            for (to, carry, pre_fader, gain, w) in routes {
                let delay = (longest - w) as usize;
                let e = edges.len();
                let mute = match carry {
                    Carry::Send(k) => t.sends[k].mute,
                    _ => false,
                };
                edges.push(Edge {
                    from: i,
                    to,
                    carry,
                    pre_fader,
                    gain,
                    last: gain,
                    mute,
                    line: [vec![0.0; delay], vec![0.0; delay]],
                    at: 0,
                });
                if !matches!(carry, Carry::Sidechain(_)) {
                    tracks[i].outgoing.push(e);
                }
                match to {
                    None => to_master.push(e),
                    Some(j) => {
                        tracks[j].incoming.push(e);
                        if let Carry::Sidechain(k) = carry {
                            tracks[j].inserts[k].key = Some(e);
                        }
                    }
                }
            }
        }
        for (t, d) in tracks.iter_mut().zip(&downstream) {
            t.latency += d;
            t.cue_line = CueLine::new(*d as usize);
        }

        let mut g = Graph {
            tempo: self.tempo.clone(),
            scratch: (0..n)
                .map(|_| [vec![0.0; max_block], vec![0.0; max_block]])
                .collect(),
            pre: (0..n)
                .map(|_| [vec![0.0; max_block], vec![0.0; max_block]])
                .collect(),
            post: (0..n)
                .map(|_| [vec![0.0; max_block], vec![0.0; max_block]])
                .collect(),
            key: [vec![0.0; max_block], vec![0.0; max_block]],
            aux,
            aux_routed,
            edges,
            to_master,
            order,
            tracks,
            master: db_to_gain(self.master_db),
            any_solo: false,
            groups: self.groups.clone(),
            clicks: Clicks::new(rate),
            streams,
            placements,
            max_block,
        };
        g.refresh_solo();
        g.settle_at(0);
        Ok(g)
    }
}

impl Graph {
    pub fn tempo(&self) -> &TempoMap {
        &self.tempo
    }

    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// The frame where the last clip ends, for offline renders.
    /// The largest block this graph was compiled for.
    pub fn max_block(&self) -> usize {
        self.max_block
    }

    pub fn end(&self) -> Frames {
        Frames(
            self.tracks
                .iter()
                .flat_map(|t| (t.clips.iter().map(|c| c.end)).chain(t.notes.iter().map(|n| n.end)))
                .max()
                .unwrap_or(0),
        )
    }

    pub(crate) fn set_mix(&mut self, track: usize, mix: Mix) {
        if let Some(t) = self.tracks.get_mut(track) {
            t.mix = mix;
        }
        self.refresh_solo();
    }

    pub(crate) fn set_group(&mut self, group: usize, spec: GroupSpec) {
        if let Some(g) = self.groups.get_mut(group) {
            *g = spec;
        }
    }

    /// Changes a send's level or mute without a new graph.
    pub(crate) fn set_send(&mut self, track: usize, send: usize, gain_db: f32, mute: bool) {
        let gain = if mute { 0.0 } else { db_to_gain(gain_db) };
        if let Some(e) = self
            .edges
            .iter_mut()
            .find(|e| e.from == track && e.carry == Carry::Send(send))
        {
            e.gain = gain;
            e.mute = mute;
        }
    }

    /// Starts each track at the gains it has at `pos`, lanes read, so
    /// the graph does not fade in when it starts playing there.
    pub(crate) fn settle_at(&mut self, pos: i64) {
        let any_solo = self.any_solo;
        for ti in 0..self.tracks.len() {
            let t = &mut self.tracks[ti];
            if !t.automation.is_empty() {
                let mut sends = [(0usize, 0f32); MAX_SEND_LANES];
                let mut count = 0;
                t.read_lanes(pos, &mut sends, &mut count);
                for &(k, db) in &sends[..count] {
                    for &e in &t.outgoing {
                        let e = &mut self.edges[e];
                        if e.carry == Carry::Send(k) {
                            e.gain = if e.mute { 0.0 } else { db_to_gain(db) };
                            e.last = e.gain;
                        }
                    }
                }
            }
            t.last = t.target(any_solo, &self.groups, true);
            t.cue_last = std::array::from_fn(|k| t.cues[k].gains(t.stereo));
        }
    }

    /// Marks a lane as being written (its control plays, not its line)
    /// or played again, without a new graph.
    pub(crate) fn set_auto_live(&mut self, track: usize, target: Target, live: bool) {
        if let Some(t) = self.tracks.get_mut(track) {
            for l in t.automation.iter_mut().filter(|l| l.target == target) {
                l.live = live;
            }
        }
    }

    /// Works out which tracks soloing leaves audible: the soloed ones,
    /// and every bus they are routed into, so soloing a track through a
    /// bus still sounds.
    fn refresh_solo(&mut self) {
        self.any_solo = self.tracks.iter().any(|t| t.mix.solo);
        for k in 0..self.order.len() {
            let i = self.order[k];
            let mut heard = self.tracks[i].mix.solo;
            for &e in &self.tracks[i].incoming {
                let e = &self.edges[e];
                if !matches!(e.carry, Carry::Sidechain(_)) && self.tracks[e.from].heard {
                    heard = true;
                }
            }
            self.tracks[i].heard = heard;
        }
    }

    /// Plays frames `[pos, pos + n)` of every track into its scratch
    /// buffers, adds monitored input, applies the mixer, and adds the
    /// tracks into `out`. With `play` false (stopped, or counting in) only
    /// monitored input sounds. `looping` is the loop region while
    /// playing: effects' latency compensation reads ahead round it.
    /// Levels go to `levels` (track by track, after the fader), and the
    /// track `tap` names is copied out to its buffers for the meters.
    /// Returns how many frames of streamed audio were missing.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render(
        &mut self,
        pos: i64,
        looping: Option<(i64, i64)>,
        out: [&mut [f32]; 2],
        levels: &mut [Level],
        input: InputBlock,
        play: bool,
        rate: Option<Rate>,
        mut tap: Option<(usize, &mut [f32], &mut [f32])>,
        live: &[Event],
        mut midi_out: Option<MidiOut<'_>>,
    ) -> usize {
        let n = out[0].len();
        let mut missing = 0;
        let [out_l, out_r] = out;
        let any_solo = self.any_solo;
        let groups = &self.groups;
        for k in 0..self.order.len() {
            let ti = self.order[k];
            let t = &mut self.tracks[ti];
            let [sl, sr] = &mut self.scratch[ti];
            let (sl, sr) = (&mut sl[..n], &mut sr[..n]);
            sl.fill(0.0);
            sr.fill(0.0);
            // What other tracks route here, each delayed to line up.
            for &e in &t.incoming {
                let e = &mut self.edges[e];
                if matches!(e.carry, Carry::Sidechain(_)) {
                    continue;
                }
                if let Carry::Aux(k) = e.carry {
                    if let Some([l, r]) = self.aux[e.from].get(k as usize - 1) {
                        e.feed((&l[..n], &r[..n]), (&mut *sl, &mut *sr));
                    }
                    continue;
                }
                let from = if e.pre_fader {
                    &self.pre[e.from]
                } else {
                    &self.post[e.from]
                };
                e.feed((&from[0][..n], &from[1][..n]), (&mut *sl, &mut *sr));
            }
            if t.monitor
                && let Some(inp) = t.input
            {
                let right = if inp.stereo { inp.first + 1 } else { inp.first };
                for i in 0..n {
                    sl[i] += input.get(i, inp.first);
                    sr[i] += input.get(i, right);
                }
            }
            if play {
                match &rate {
                    Some(rate) => {
                        let st = &mut t.stretch;
                        st.set(rate.speed, rate.semitones);
                        if rate.reset {
                            // Effects' latency: read that far ahead, in
                            // input time.
                            st.reset(rate.u + t.latency as f64 * rate.speed);
                        }
                        let mut feed = TrackFeed {
                            clips: &t.clips,
                            streams: &self.streams,
                            looping,
                            missing: 0,
                        };
                        st.process(&mut feed, sl, sr);
                        missing += feed.missing;
                    }
                    // Read ahead by the effects' latency, round the loop
                    // when that runs past its end.
                    None => {
                        missing +=
                            read_looped(&t.clips, &self.streams, looping, pos + t.latency, sl, sr)
                    }
                }
            }

            // Notes: those from clips for this block, then the live
            // ones, to the instrument and the MIDI output.
            t.events.clear();
            if play {
                let (from, speed) = match &rate {
                    Some(r) => (r.u + t.latency as f64 * r.speed, r.speed),
                    None => ((pos + t.latency) as f64, 1.0),
                };
                schedule_notes(
                    &t.notes,
                    &t.exprs,
                    &t.controls,
                    &mut t.sounding,
                    &mut t.notes_at,
                    &mut t.pass,
                    looping,
                    from,
                    speed,
                    n,
                    &mut t.events,
                );
            } else if t.was_playing {
                t.sounding.clear();
                t.notes_at = None;
                t.events.push(Event::all_notes_off(0));
            }
            t.was_playing = play;
            if t.notes_live {
                for e in live {
                    if t.events.len() < MAX_EVENTS {
                        t.events.push(*e);
                    }
                }
            }
            let shift = rate.as_ref().map_or(0, |r| r.semitones.round() as i32);
            if shift != 0 {
                for e in &mut t.events {
                    *e = e.transposed(shift);
                }
            }
            t.events.sort_by_key(|e| e.frame);
            if let Some(port) = t.midi_out
                && let Some(out) = midi_out.as_mut()
            {
                for e in &t.events {
                    out(port, e);
                }
            }
            let aux = &mut self.aux[ti];
            for [l, r] in aux.iter_mut() {
                l[..n].fill(0.0);
                r[..n].fill(0.0);
            }
            if !t.layers.is_empty() {
                let mut keys = chain::Keys {
                    edges: &mut self.edges,
                    post: &self.post,
                    key: &mut self.key,
                };
                layers::run(
                    &mut t.layers,
                    &t.events,
                    shift,
                    &mut t.inserts,
                    &mut keys,
                    sl,
                    sr,
                );
            } else if let Some(InstrumentNode {
                inst: Some(inst), ..
            }) = &mut t.instrument
            {
                let mut outs = Outputs::new(aux, &self.aux_routed[ti], n);
                inst.process_outputs(&t.events, sl, sr, &mut outs);
            }

            if t.widen {
                sr.copy_from_slice(sl);
            }
            // The lanes at the block's end: the fader ramps to it over
            // the block, which is the line itself when it is straight.
            if !t.automation.is_empty() {
                let mut sends = [(0usize, 0f32); MAX_SEND_LANES];
                let mut count = 0;
                t.read_lanes(pos + n as i64, &mut sends, &mut count);
                for &(k, db) in &sends[..count] {
                    for &e in &t.outgoing {
                        let e = &mut self.edges[e];
                        if e.carry == Carry::Send(k) {
                            e.gain = if e.mute { 0.0 } else { db_to_gain(db) };
                        }
                    }
                }
            }
            let mut keys = chain::Keys {
                edges: &mut self.edges,
                post: &self.post,
                key: &mut self.key,
            };
            chain::run(&mut t.chain, &mut t.inserts, &mut keys, sl, sr);

            let target = t.target(any_solo, groups, true);
            let dry = t.target(any_solo, groups, false);
            // Ramp linearly from last block's gains so changes don't click.
            let (l0, r0) = t.last;
            let step = 1.0 / n.max(1) as f32;
            let mut peak = (0.0f32, 0.0f32);
            let mut sq = (0.0f32, 0.0f32);
            let mut tap = tap
                .as_mut()
                .filter(|t| t.0 == ti)
                .map(|(_, l, r)| (&mut **l, &mut **r));
            let [pre_l, pre_r] = &mut self.pre[ti];
            let [post_l, post_r] = &mut self.post[ti];
            for i in 0..n {
                let k = (i + 1) as f32 * step;
                let gl = l0 + (target.0 - l0) * k;
                let gr = r0 + (target.1 - r0) * k;
                // A mono source feeds both sides of the pan.
                let (a, b) = if t.stereo {
                    (sl[i], sr[i])
                } else {
                    (sl[i], sl[i])
                };
                pre_l[i] = a * dry.0;
                pre_r[i] = b * dry.1;
                let (a, b) = (a * gl, b * gr);
                peak = (peak.0.max(a.abs()), peak.1.max(b.abs()));
                sq = (sq.0 + a * a, sq.1 + b * b);
                if let Some((tl, tr)) = &mut tap {
                    tl[i] = a;
                    tr[i] = b;
                }
                post_l[i] = a;
                post_r[i] = b;
            }
            t.last = target;
            if let Some(l) = levels.get_mut(ti) {
                *l = Level { peak, sq };
            }
        }
        for &e in &self.to_master {
            let e = &mut self.edges[e];
            let from = &self.post[e.from];
            e.feed((&from[0][..n], &from[1][..n]), (&mut *out_l, &mut *out_r));
        }
        for s in out_l.iter_mut().chain(out_r.iter_mut()) {
            *s *= self.master;
        }
        missing
    }

    /// The streamed clips, for [`Streamer::set_placements`](crate::stream::Streamer::set_placements).
    pub fn placements(&self) -> Vec<Placement> {
        self.placements.clone()
    }

    /// Stores a chunk from the disk thread. Gives it back if no stream in
    /// this graph wants it.
    pub(crate) fn insert_chunk(&mut self, c: Box<Chunk>) -> Option<Box<Chunk>> {
        let Some(s) = self.streams.iter_mut().find(|s| s.id == c.source) else {
            return Some(c);
        };
        match s.slots.get_mut(c.index) {
            Some(slot) => slot.replace(c),
            None => Some(c),
        }
    }

    pub(crate) fn take_chunk(&mut self, source: u64, index: usize) -> Option<Box<Chunk>> {
        self.streams
            .iter_mut()
            .find(|s| s.id == source)?
            .slots
            .get_mut(index)?
            .take()
    }

    /// Takes over the effects of `old` that this graph still uses. The
    /// rest stay in `old`, to be dropped with it off the audio thread.
    /// Takes over the stretchers of `old`, track by track, so a graph
    /// change while playing slowed does not restart them.
    pub(crate) fn adopt_stretchers(&mut self, old: &mut Graph) {
        for (t, o) in self.tracks.iter_mut().zip(&mut old.tracks) {
            std::mem::swap(&mut t.stretch, &mut o.stretch);
        }
    }

    /// Takes over the delay lines of `old` for routes this graph still
    /// has with the same delay, so a graph change does not drop the
    /// audio in flight on them.
    pub(crate) fn adopt_edges(&mut self, old: &mut Graph) {
        for e in &mut self.edges {
            if let Some(o) = old.edges.iter_mut().find(|o| {
                o.from == e.from && o.to == e.to && o.carry == e.carry && o.delay() == e.delay()
            }) {
                std::mem::swap(&mut e.line, &mut o.line);
                e.at = o.at;
                e.last = o.last;
            }
        }
    }

    /// Takes over the instruments of `old` that this graph still uses,
    /// with the notes they have sounding, so a graph change while
    /// playing cuts nothing off.
    pub(crate) fn adopt_instruments(&mut self, old: &mut Graph) {
        for t in &mut self.tracks {
            // The old track the first instrument taken over came from.
            let mut from = None;
            if let Some(i) = t.instrument.as_mut().filter(|i| i.inst.is_none())
                && let Some((k, inst)) = take_instrument(old, i.id, None)
            {
                i.inst = Some(inst);
                from = Some(k);
            }
            for l in &mut t.layers {
                let Some(i) = l.instrument.as_mut().filter(|i| i.inst.is_none()) else {
                    continue;
                };
                if let Some((k, inst)) = take_instrument(old, i.id, Some(&mut l.held)) {
                    i.inst = Some(inst);
                    from.get_or_insert(k);
                }
            }
            let Some(k) = from else { continue };
            let o = &old.tracks[k];
            // The notes sounding carry on where this graph has the same
            // note; one that is gone is stopped now.
            t.notes_at = o.notes_at;
            t.pass = o.pass;
            t.was_playing = o.was_playing;
            for &k in &o.sounding {
                let on = &o.notes[k];
                let same = t.notes.iter().position(|nn| {
                    (nn.start, nn.end, nn.pitch, nn.channel)
                        == (on.start, on.end, on.pitch, on.channel)
                });
                match same {
                    Some(j) if t.sounding.len() < t.sounding.capacity() => t.sounding.push(j),
                    _ => t
                        .events
                        .push(Event::note_off(0, on.channel, on.pitch, k as u32)),
                }
            }
            if !t.events.is_empty() {
                if let Some(InstrumentNode {
                    inst: Some(inst), ..
                }) = &mut t.instrument
                {
                    inst.process(&t.events, &mut [], &mut []);
                }
                layers::stop(&mut t.layers, &t.events);
                t.events.clear();
            }
        }
    }

    pub(crate) fn adopt_inserts(&mut self, old: &mut Graph) {
        for t in &mut self.tracks {
            for slot in t.inserts.iter_mut().filter(|s| s.effect.is_none()) {
                slot.effect = old
                    .tracks
                    .iter_mut()
                    .flat_map(|o| o.inserts.iter_mut())
                    .find(|o| o.id == slot.id)
                    .and_then(|o| o.effect.take());
            }
        }
    }

    /// Moves every loaded chunk out of `old` into this graph where it has
    /// a slot for it; `give_back` gets the rest.
    pub(crate) fn adopt_chunks(&mut self, old: &mut Graph, mut give_back: impl FnMut(Box<Chunk>)) {
        for s in &mut old.streams {
            for slot in &mut s.slots {
                if let Some(c) = slot.take()
                    && let Some(c) = self.insert_chunk(c)
                {
                    give_back(c);
                }
            }
        }
    }
}

/// Takes the instrument with this id out of `old`, from a track or a
/// layer, with the old track's index. Going to a layer (`held`), it
/// takes over what the old layer held; one that changes from a track's
/// instrument to a layer's, or back, has its notes stopped, as which
/// layer started which can't be told.
fn take_instrument(
    old: &mut Graph,
    id: u64,
    held: Option<&mut Box<Held>>,
) -> Option<(usize, Box<dyn Instrument>)> {
    for (k, o) in old.tracks.iter_mut().enumerate() {
        if let Some(mut inst) = (o.instrument.as_mut())
            .filter(|x| x.id == id)
            .and_then(|x| x.inst.take())
        {
            if held.is_some() {
                inst.process(&[Event::all_notes_off(0)], &mut [], &mut []);
            }
            return Some((k, inst));
        }
        for l in &mut o.layers {
            if let Some(mut inst) = (l.instrument.as_mut())
                .filter(|x| x.id == id)
                .and_then(|x| x.inst.take())
            {
                match held {
                    Some(h) => std::mem::swap(h, &mut l.held),
                    None => inst.process(&[Event::all_notes_off(0)], &mut [], &mut []),
                }
                return Some((k, inst));
            }
        }
    }
    None
}
