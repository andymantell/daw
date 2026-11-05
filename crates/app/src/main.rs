// SPDX-License-Identifier: GPL-3.0-or-later
//! The desktop application. Phase 1 so far: open audio files, and the
//! sound of videos, as tracks; play them with a mixer, loop, metronome
//! and count-in, choose the audio device, and record: per track input, arm and monitor, with takes kept
//! and switchable; VST 3 effects as track inserts; a tuner and practice
//! mute; export of the mix and stems to WAV, FLAC, MP3 or Opus; a timeline to edit clips
//! and mark sections; comments pinned to times in the song; level,
//! loudness and spectrum meters.
//!
//! Usage: `daw [file ...]`, or drop audio or video files on the window:
//! WAV, MP3, FLAC, OGG, AAC/M4A, AIFF, or the sound of an MP4, MOV or MKV
//! video. Files in other formats or at another sample rate from the
//! device are converted in the background into a cache folder first.
//!
//! `daw --version` prints the version.
//!
//! `daw --headless` runs the app with no window, for AI assistants and
//! tests to drive (see `headless.rs`).
//!
//! `daw --scan-vst3 <bundle>` is the plugin scanner: the app runs itself
//! that way to open each plugin safely in a separate process.

mod agent;
mod agent_arrange;
mod agent_automation;
mod agent_device;
mod agent_edit;
mod agent_files;
mod agent_layers;
mod agent_lessons;
mod agent_midi;
mod agent_monitor;
mod agent_notes;
mod agent_pitch;
mod agent_plugins;
mod agent_practice;
mod agent_routing;
mod agent_takes;
mod assistants;
mod audio_setup;
mod automation;
mod beat_grid;
mod blue_wave_panel;
mod browser;
mod commands;
mod comments;
mod comping;
mod crash;
mod desk;
mod export;
mod freeze;
mod fretboard;
mod fx;
mod fx_panel;
mod groups;
mod headless;
mod history;
mod hitpoints;
mod icons;
mod import_project;
mod inspector;
mod instrument;
mod interchange;
mod kits;
mod latency;
mod layers;
mod lessons;
mod linked;
mod listen;
mod lucky_seven_panel;
mod lyrics;
mod menus;
mod meters;
mod midi;
mod midifile;
mod mixer;
mod monitor;
mod notation;
mod note_tools;
mod notefx;
mod notes;
mod notes_editor;
mod panel_kit;
mod patch_twenty;
mod patch_twenty_panel;
mod patterns;
mod people;
mod perf;
mod pitch_edit;
mod practice;
mod presets;
mod process;
mod project;
mod recording;
mod remote;
mod render;
mod review;
mod sampler_panel;
mod sections;
mod settings;
mod share;
mod simple_mode;
mod slicer_panel;
mod snapshots;
mod spoken;
mod start;
mod stretch;
mod switch;
mod synth_panel;
mod templates;
mod tempo;
mod theme;
mod timeline;
mod timing;
mod top_bar;
mod track_panes;
mod trainer;
mod transpose;
mod tuner;
mod update;
mod versions;
mod video;
#[cfg(test)]
mod widget_check;
mod workspace;

#[cfg(test)]
mod a11y;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use commands::Cmd;
use daw_audio_io::{self as audio, DeviceProfile, HostInfo, Running, SavedProfiles};
use daw_engine::engine::{self, Command, Controller};
use daw_engine::graph::{ClipSpec, Input, Mix, Output, Session, TrackSpec};
use daw_engine::import;
use daw_engine::metronome::Metronome;
use daw_engine::record::{RecorderThread, Take};
use daw_engine::stream::{StreamSource, StreamerThread};
use daw_engine::time::{Frames, TempoMap};
use daw_project::TrackKind;
use eframe::egui::{self, RichText};
use fx::{Effects, FxSlot};
use timeline::{Clip, Lane, Marker, Song};

struct Track {
    /// The track's id in the project document.
    key: String,
    name: String,
    /// Imported files and recorded takes, in the order added (later ones
    /// draw on top).
    clips: Vec<Clip>,
    mix: Mix,
    input: Option<Input>,
    arm: bool,
    monitor: bool,
    /// Silenced to play the part yourself. Kept apart from `mix.mute`,
    /// which belongs to the mix.
    practice_mute: bool,
    /// Note clips (roadmap 3.1), in ticks, and the instrument that
    /// plays them on an instrument track.
    note_clips: Vec<daw_project::NoteClip>,
    instrument: Option<instrument::Slot>,
    /// Where the instrument's extra outputs go: a drum machine's pads
    /// on buses of their own.
    instrument_outputs: Vec<daw_project::InstrumentOutput>,
    /// Layered instruments (roadmap 3.5): when there are any, they play
    /// the notes in place of `instrument`, which is then `None`.
    layers: Vec<layers::Layer>,
    /// A MIDI output port the track's notes go to as well (this
    /// computer's; not saved with the project).
    midi_out: Option<usize>,
    /// Note-repeat (roadmap 3.3): held live notes play again at this
    /// rate, while on. Kept for this run only, like monitoring.
    note_repeat: Option<daw_project::StepRate>,
    /// Note effects (Phase 4, A6), as in the project: the notes go
    /// through them before the instrument.
    note_fx: Vec<daw_project::NoteFx>,
    /// How its units show in the rack panel (collapsed or hidden ones).
    rack: Vec<daw_project::RackUnit>,
    /// Insert effects, in order.
    fx: Vec<FxSlot>,
    /// The track rendered through its effects, as the document has it.
    frozen: Option<daw_project::Frozen>,
    /// Its audio, opened when one of the track's plugins is missing here.
    frozen_audio: Option<Arc<StreamSource>>,
    /// Frozen by the user: plays its frozen audio, effects off.
    freeze: bool,
    /// Peak levels shown on the meter, decaying.
    meter: (f32, f32),
    /// Comps made from the track's takes, and the one that plays.
    comps: Vec<daw_project::Comp>,
    comp: Option<String>,
    /// Audio track, bus or folder (see [`daw_project::TrackKind`]).
    kind: TrackKind,
    /// The folder it sits in, by key.
    parent: Option<String>,
    /// The bus its audio goes to, by key: `None` for its folder, or the
    /// master when it has none.
    output: Option<String>,
    sends: Vec<daw_project::Send>,
    /// The control group it belongs to, by key.
    group: Option<String>,
    /// Automation lanes, with positions at the app's rate.
    automation: Vec<daw_project::Lane>,
    /// Its notes shown as tab or a staff under it (roadmap 3.8).
    notation: Option<daw_project::Notation>,
    /// Its other versions, as the document has them, and the one playing
    /// (see [`daw_project::Track::versions`]).
    versions: Vec<daw_project::TrackVersion>,
    version: Option<String>,
}

/// Fresh copies of the master's effects in `master_fx` for the inserts
/// `s` keeps, with the latency each reports (see
/// [`App::fresh_master_fx`]).
fn fresh_master_fx(
    effects: &mut fx::Effects,
    master_fx: &[FxSlot],
    rate: u32,
    s: &mut Session,
    plugins: &mut Vec<daw_plugins::Plugin>,
    fx: &mut Vec<(u64, Box<dyn daw_engine::graph::Insert>)>,
) -> Result<(), String> {
    for i in &mut s.master_inserts {
        let Some(slot) = master_fx.iter().find(|f| f.id == i.id) else {
            continue;
        };
        let (p, proc) = effects.fresh(slot, rate)?;
        if let Some(p) = &p {
            i.latency = p.latency();
        }
        plugins.extend(p);
        fx.push((slot.id, proc));
    }
    Ok(())
}

impl Track {
    /// The master channel as the app keeps it: a track by the id
    /// assistants use, of which only the effects and fader count.
    fn master() -> Track {
        let mut t = Track::new("Master".into());
        t.key = daw_project::Master::KEY.to_string();
        t.kind = TrackKind::Bus;
        t
    }

    /// The mix the engine plays: the track's own, with practice mute.
    fn heard(&self) -> Mix {
        Mix {
            mute: self.mix.mute || self.practice_mute,
            ..self.mix
        }
    }

    fn new(name: String) -> Track {
        Track {
            key: daw_project::new_id(),
            name,
            clips: Vec::new(),
            mix: Mix::default(),
            input: None,
            arm: false,
            monitor: false,
            practice_mute: false,
            note_clips: Vec::new(),
            instrument: None,
            instrument_outputs: Vec::new(),
            layers: Vec::new(),
            midi_out: None,
            note_repeat: None,
            note_fx: Vec::new(),
            rack: Vec::new(),
            fx: Vec::new(),
            frozen: None,
            frozen_audio: None,
            freeze: false,
            meter: (0.0, 0.0),
            comps: Vec::new(),
            comp: None,
            kind: TrackKind::Audio,
            parent: None,
            output: None,
            sends: Vec::new(),
            group: None,
            automation: Vec::new(),
            notation: None,
            versions: Vec::new(),
            version: None,
        }
    }

    /// Where its audio goes: the bus it is routed to, else its folder,
    /// else (`None`) the master.
    fn goes_to(&self) -> Option<&str> {
        self.output.as_deref().or(self.parent.as_deref())
    }

    /// An instrument track with the simple synth.
    fn instrument(name: String) -> Track {
        let mut t = Track::new(name);
        t.kind = TrackKind::Instrument;
        t.instrument = Some(instrument::Slot::synth());
        t.monitor = true;
        t
    }

    /// Every instrument it has: its own, or its layers'.
    fn instruments(&self) -> impl Iterator<Item = &instrument::Slot> {
        (self.instrument.iter()).chain(self.layers.iter().map(|l| &l.slot))
    }

    fn instruments_mut(&mut self) -> impl Iterator<Item = &mut instrument::Slot> {
        (self.instrument.iter_mut()).chain(self.layers.iter_mut().map(|l| &mut l.slot))
    }

    /// The track's notes as the engine plays them, with their
    /// expression, and its controller lanes.
    fn note_specs(
        &self,
    ) -> (
        Vec<daw_engine::graph::NoteSpec>,
        Vec<daw_engine::graph::ExprSpec>,
        Vec<daw_engine::graph::ControlSpec>,
    ) {
        if !self.kind.plays_notes() {
            return Default::default();
        }
        note_tools::specs(&self.note_clips)
    }
}

/// A file being opened (and converted, if needed) on a thread of its own.
struct Import {
    path: PathBuf,
    /// Opened again at a new rate for the clips already using it, rather
    /// than added as a track.
    reopen: bool,
    /// Where a new track's clip starts.
    at: i64,
    rate: u32,
    /// The file the clip plays (a video's sound is taken out into one of
    /// its own), opened.
    done: mpsc::Receiver<Result<(PathBuf, Arc<StreamSource>), String>>,
}

/// How the engine plays a clip under the song's tempo map: through the
/// stretcher along its warp when it follows tempo or has markers, or
/// when it is transposed.
fn clip_spec(c: &Clip, map: &TempoMap) -> ClipSpec {
    let mut warp = c.warp(map).map(Arc::new);
    // Notes moved in time by pitch editing: a warp inside the clip's own.
    if let Some(inner) = c.note_warp() {
        let range = c.offset as i64..(c.offset + c.len) as i64;
        let outer = match &warp {
            Some(w) => Some(w.as_ref().clone()),
            None => daw_engine::warp::Warp::even(range, c.start, 1.0).ok(),
        };
        if let Some(w) = outer.and_then(|o| o.then(&inner).ok()) {
            warp = Some(Arc::new(w));
        }
    }
    let bend = c.pitch_bend().map(Arc::new);
    let stretched = warp.is_some() || c.cents != 0 || bend.is_some();
    ClipSpec {
        offset: c.offset,
        len: Some(c.len),
        gain_db: c.gain_db,
        fade_in: c.fade_in,
        fade_out: c.fade_out,
        envelope: c.envelope.as_slice().into(),
        tag: if stretched { c.id } else { 0 },
        warp,
        semitones: c.semitones(),
        formants: !c.move_formants,
        bend,
        ..ClipSpec::new(Frames(c.start), c.source.clone())
    }
}

/// Where converted copies of files go until projects have folders of
/// their own.
pub(crate) fn cache_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_CACHE_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("daw").join("converted")
}

/// The product's name, until it has a real one: the window title and the
/// folder for unsaved takes follow it.
pub(crate) const APP_NAME: &str = "DAW";

/// Where takes go until the project is saved, when they move into the
/// project's folder: a visible folder in the user's profile (the owner's
/// choice, 2026-10-27), e.g. `C:\Users\<name>\DAW\Unsaved takes`.
fn takes_dir() -> PathBuf {
    user_dir().join(APP_NAME).join("Unsaved takes")
}

/// The user's own folder: `C:\Users\<name>` on Windows.
fn user_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map_or_else(std::env::temp_dir, PathBuf::from)
}

/// What the audio side is doing.
enum Audio {
    Running(Running),
    /// No device could be opened, so the engine runs without sound; the
    /// message says why. The `Running` is held to keep it going.
    Silent(Running, String),
    /// Between devices.
    Stopped,
}

struct App {
    tracks: Vec<Track>,
    /// The master channel: the whole mix's effects and fader, kept as a
    /// track so the effects window and settings work on it alike. Only
    /// its `fx` and `mix.gain_db` count.
    master: Track,
    /// Control groups: one fader and mute over several tracks.
    groups: Vec<daw_project::ControlGroup>,
    /// Headphone mixes and the speakers' controls.
    cues: monitor::Cues,
    imports: Vec<Import>,
    tempo_bpm: f64,
    beats_per_bar: u32,
    /// Tempo and time-signature changes, and the start's beat unit.
    time: tempo::Changes,
    /// The tempo map changed: clips that follow tempo move to where
    /// their beat now falls at the next rebuild, instead of their beat
    /// following where they are.
    retime: bool,
    tempo_ui: tempo::TempoWindow,
    rate: u32,
    ctl: Controller,
    _disk: StreamerThread,
    _recorder: RecorderThread,
    recording: bool,
    /// Punch range, pre-roll and post-roll, and retrospective capture.
    record: recording::RecordOptions,
    audio: Audio,
    profile: Option<DeviceProfile>,
    /// The settings being edited in the device window.
    draft: Option<DeviceProfile>,
    hosts: Vec<HostInfo>,
    /// Devices on the draft's driver that can record.
    input_devices: Vec<String>,
    show_devices: bool,
    settings: settings::Settings,
    metronome: Metronome,
    count_in: bool,
    looping: bool,
    /// Bars, counted from 1: loop from the start of `loop_bars.0` to the
    /// start of `loop_bars.1`.
    loop_bars: (i64, i64),
    master_meter: (f32, f32),
    message: Option<String>,
    /// The words of the value last heard (Phase 4 D3).
    said: spoken::Said,
    /// The device stopped (unplugged, say): when to look for it again.
    reconnect: Option<Instant>,
    /// The mix buffer last turned on, for turning it on again.
    last_mix_buffer: Option<u32>,
    /// Device settings kept between runs.
    saved: SavedProfiles,
    effects: Effects,
    tuner: tuner::Tuner,
    /// Electronic drum kit profiles and the learn wizard.
    kits: kits::Kits,
    meters: meters::Meters,
    export: export::Export,
    freezer: freeze::Freezer,
    processing: process::Processing,
    /// Renders in place, bounces and pastes as rendered audio, and the
    /// copied clips.
    renders: render::Renders,
    /// Practice speed, percent, and transpose.
    speed_pct: f64,
    semitones: i32,
    cents: i32,
    trainer: trainer::Trainer,
    top: top_bar::TopBar,
    /// A track's tuning being measured, and the last result.
    detecting: Option<(String, mpsc::Receiver<Option<f32>>)>,
    detected: Option<(String, Option<f32>)>,
    markers: Vec<Marker>,
    /// How the song lines up with a picture's timecode, and the hit
    /// points on it (positions at the device's rate), and their window.
    timecode: daw_project::TimecodeSetup,
    hit_points: Vec<daw_project::HitPoint>,
    hits_ui: hitpoints::HitPointsWindow,
    time_ui: sections::TimeWindow,
    sections_ui: sections::SectionsWindow,
    /// The order the song's sections play in, by marker id.
    arrangement: Vec<String>,
    arranging: Option<sections::Arranging>,
    /// The song's key and scale, its groove templates and its MIDI
    /// learn ties, as in the project, and the note tools' settings.
    scale: Option<daw_project::Scale>,
    grooves: Vec<daw_project::Groove>,
    midi_maps: Vec<daw_project::MidiMap>,
    note_tools: note_tools::NoteTools,
    /// Notation lanes under tracks (roadmap 3.8).
    notation: notation::Lanes,
    timeline: timeline::Timeline,
    perf: perf::Perf,
    /// Lesson notes and tab blocks, as in the project, and their window.
    notes: String,
    tabs: Vec<daw_project::TabBlock>,
    notes_ui: notes::Notes,
    /// Assignments, as in the project, the Lessons pane, and the
    /// practice session going on now.
    assignments: Vec<daw_project::Assignment>,
    lessons_ui: lessons::Lessons,
    practice_log: practice::Logger,
    /// Tracks edited together, as in the project, and their window.
    edit_groups: Vec<daw_project::EditGroup>,
    groups_ui: groups::Groups,
    /// The Fix timing window.
    timing: timing::Timing,
    /// Audio that follows tempo: its windows and settings.
    stretch: stretch::Stretch,
    transpose: transpose::Transpose,
    /// Pitch editing: lines worked out, the notes chosen, a drag.
    pitch: pitch_edit::PitchEdit,
    /// Finding the words sung in vocal clips, and the speech model.
    lyrics: lyrics::Lyrics,
    takes_ui: comping::TakesPane,
    /// Automation lanes: their pane, modes and the passes being written.
    automation: automation::Automation,
    /// Pinned comments, as in the project, and their panel.
    comments: Vec<daw_project::Comment>,
    comment_ui: comments::Comments,
    /// The snapshots window.
    snapshot_ui: snapshots::Snapshots,
    /// The undo list and activity window.
    history: history::History,
    review: review::Review,
    versions_ui: versions::Versions,
    /// The New from template and Save as template windows.
    template_ui: templates::Templates,
    preset_ui: presets::Presets,
    /// The Import from project and Export parts windows.
    parts: import_project::Parts,
    /// The start screen, and the recent projects it lists.
    start: start::Start,
    /// Named commands, their keys, and their windows.
    commands: commands::Commands,
    simple: simple_mode::Simple,
    /// How the mixer desk is shown: narrow strips, and which are selected.
    desk: mixer::DeskView,
    /// The inspector's track, and what it last saw selected.
    inspector: inspector::Inspector,
    track_panes: track_panes::Follow,
    proj: project::Proj,
    latency: latency::Latency,
    /// A simulated loopback cable's delay for measuring in tests.
    #[cfg(test)]
    loopback_for_tests: Option<u32>,
    /// The guided audio setup.
    audio_setup: audio_setup::AudioSetup,
    updates: update::Updates,
    crashes: crash::Crashes,
    /// This install's identity, and the people windows.
    me: daw_project::Identity,
    people: people::People,
    /// The phone remote: its window and the phones connected.
    remote: remote::Remote,
    /// The panes, and the layouts saved on this computer.
    workspace: workspace::Workspace,
    /// The panes are being drawn, so the workspace is out of `self`.
    drawing_panes: bool,
    /// Workspace commands that came while it was out.
    workspace_cmds: Vec<Cmd>,
    /// Panes asked for while the panes were drawn, opened after.
    panes_to_open: Vec<workspace::Pane>,
    /// The look, chosen per computer.
    themes: theme::Themes,
    /// MIDI devices, and notes being recorded.
    midi: midi::Midi,
    /// Instruments made and not yet handed to the engine, by id.
    pending_instruments: Vec<(u64, Box<dyn daw_engine::midi::Instrument>)>,
    /// The note editor pane, and the track whose instrument window is
    /// open.
    notes_editor: notes_editor::Editor,
    /// Drum patterns that note clips play (roadmap 3.3).
    patterns: Vec<daw_project::Pattern>,
    pattern_editor: patterns::Editor,
    instrument_open: Option<usize>,
    /// The content browser, and whether a preview was just sent to the
    /// engine (so its silence a moment ago doesn't mean it finished).
    browser: browser::Browser,
    browser_sent: bool,
    /// Video clips' pictures, and the Video pane.
    video: video::Videos,
    /// Tool calls from AI assistants (roadmap 2.11).
    agent: agent::Agent,
    /// Exporting for and importing from other DAWs.
    swap: interchange::Swap,
    /// Settings → AI assistants…, and the server they connect to.
    assistants: assistants::Assistants,
    /// Renders and pictures being made for assistants.
    listening: listen::Listening,
    /// The fretboard pane's settings.
    fretboard: fretboard::Fretboard,
}

/// How often to look for a lost device.
const RECONNECT_EVERY: Duration = Duration::from_secs(2);

impl App {
    fn new(files: Vec<String>) -> App {
        // The device used last time, if it is still there. Tests run
        // without settings or a device.
        let saved = if cfg!(test) {
            SavedProfiles::default()
        } else {
            load_settings()
        };
        // Headless, it plays silently.
        let quiet = cfg!(test) || headless::on();
        let profile = (saved.current())
            .filter(|p| !quiet && audio::is_present(p))
            .cloned()
            .or_else(|| (!quiet).then(audio::default_profile).flatten());
        let rate = profile.as_ref().map_or(48_000, |p| p.rate);
        let (ctl, eng, disk, recorder) = new_engine();
        let mut app = App {
            tracks: Vec::new(),
            master: Track::master(),
            groups: Vec::new(),
            cues: monitor::Cues::default(),
            imports: Vec::new(),
            tempo_bpm: 120.0,
            beats_per_bar: 4,
            time: tempo::Changes::default(),
            tempo_ui: tempo::TempoWindow::default(),
            rate,
            ctl,
            _disk: disk,
            _recorder: recorder,
            recording: false,
            record: recording::RecordOptions::default(),
            audio: Audio::Stopped,
            draft: profile.clone(),
            profile: profile.clone(),
            hosts: audio::list(),
            input_devices: input_names(&draft_host(&profile)),
            show_devices: false,
            settings: Default::default(),
            metronome: Metronome::default(),
            count_in: false,
            looping: false,
            loop_bars: (1, 5),
            master_meter: (0.0, 0.0),
            message: None,
            said: spoken::Said::default(),
            retime: false,
            reconnect: None,
            last_mix_buffer: None,
            saved,
            effects: Effects::new(config_dir().join("plugins.txt")),
            tuner: tuner::Tuner::new(rate),
            meters: meters::Meters::new(rate),
            export: export::Export::default(),
            freezer: freeze::Freezer::default(),
            processing: process::Processing::default(),
            renders: render::Renders::default(),
            speed_pct: 100.0,
            semitones: 0,
            cents: 0,
            trainer: trainer::Trainer::default(),
            top: top_bar::TopBar::default(),
            detecting: None,
            detected: None,
            markers: Vec::new(),
            timecode: daw_project::TimecodeSetup::default(),
            hit_points: Vec::new(),
            scale: None,
            grooves: Vec::new(),
            midi_maps: Vec::new(),
            note_tools: note_tools::NoteTools::default(),
            notation: notation::Lanes::default(),
            hits_ui: hitpoints::HitPointsWindow::default(),
            time_ui: sections::TimeWindow::default(),
            sections_ui: sections::SectionsWindow::default(),
            arrangement: Vec::new(),
            arranging: None,
            timeline: timeline::Timeline::default(),
            perf: perf::Perf::default(),
            notes: String::new(),
            tabs: Vec::new(),
            notes_ui: notes::Notes::default(),
            assignments: Vec::new(),
            lessons_ui: lessons::Lessons::default(),
            midi: midi::Midi::default(),
            kits: kits::Kits::new((!cfg!(test)).then(|| config_dir().join("kits"))),
            pending_instruments: Vec::new(),
            notes_editor: notes_editor::Editor::default(),
            patterns: Vec::new(),
            fretboard: fretboard::Fretboard::default(),
            pattern_editor: patterns::Editor::default(),
            instrument_open: None,
            practice_log: practice::Logger::default(),
            edit_groups: Vec::new(),
            groups_ui: groups::Groups::default(),
            timing: timing::Timing::default(),
            stretch: stretch::Stretch::default(),
            transpose: transpose::Transpose::default(),
            pitch: pitch_edit::PitchEdit::default(),
            lyrics: lyrics::Lyrics::default(),
            takes_ui: comping::TakesPane::default(),
            automation: automation::Automation::default(),
            comments: Vec::new(),
            comment_ui: comments::Comments::default(),
            snapshot_ui: snapshots::Snapshots::default(),
            history: history::History::default(),
            review: review::Review::default(),
            versions_ui: versions::Versions::default(),
            template_ui: templates::Templates::default(),
            preset_ui: presets::Presets::default(),
            parts: import_project::Parts::default(),
            start: start::Start::new((!cfg!(test)).then(|| config_dir().join("start.txt"))),
            commands: commands::Commands::new((!cfg!(test)).then(|| config_dir().join("keys.txt"))),
            simple: simple_mode::Simple::new(
                (!cfg!(test)).then(|| config_dir().join("simple.txt")),
            ),
            desk: mixer::DeskView::default(),
            inspector: inspector::Inspector::default(),
            track_panes: track_panes::Follow::default(),
            proj: project::Proj::new(),
            latency: latency::Latency::default(),
            #[cfg(test)]
            loopback_for_tests: None,
            audio_setup: audio_setup::AudioSetup::new(
                (!cfg!(test)).then(|| config_dir().join("audio-setup.txt")),
            ),
            updates: update::Updates::new((!cfg!(test)).then(|| config_dir().join("updates.txt"))),
            crashes: crash::Crashes::new(if cfg!(test) {
                std::env::temp_dir().join("daw-test-no-crash-reports")
            } else {
                crash::reports_dir()
            }),
            me: people::load_identity(),
            people: people::People::default(),
            remote: remote::Remote::load(),
            workspace: workspace::Workspace::new(
                (!cfg!(test)).then(|| config_dir().join("layout.txt")),
            ),
            drawing_panes: false,
            workspace_cmds: Vec::new(),
            panes_to_open: Vec::new(),
            themes: theme::Themes::new((!cfg!(test)).then(config_dir).as_deref()),
            browser: browser::Browser::new((!cfg!(test)).then(|| config_dir().join("browser.txt"))),
            browser_sent: false,
            video: video::Videos::default(),
            agent: agent::Agent::default(),
            swap: interchange::Swap::default(),
            assistants: assistants::Assistants::load(),
            listening: listen::Listening::default(),
        };
        if !cfg!(test) {
            app.effects.start_scan();
        }
        app.look_for_recovery();
        app.audio = app.open_device(eng);
        // First run with no device settings: the guide helps pick one.
        if app.audio_setup.first_run(!app.saved.profiles.is_empty()) {
            app.audio_setup_command(Cmd::AudioSetup);
        }
        app.midi.start(&mut app.ctl);
        app.send_capture();
        app.sign_in();
        if people::ask_name_at_start(&app.me) {
            app.ask_name();
        }
        // The empty project as it starts is not an edit.
        let start = app.project();
        app.proj.doc.set_quietly(&start);
        app.apply(start);
        // Nothing to open, so the start screen offers what to do (unless
        // turned off, or a project to recover is offered first).
        if files.is_empty() && app.proj.found.is_none() && app.start.settings.show && !cfg!(test) {
            app.show_start();
        }
        for f in files {
            let f = PathBuf::from(f);
            if project::is_project(&f) {
                app.open_project(f);
            } else if daw_collab::bundle::is_bundle(&f) {
                app.import_bundle(f);
            } else {
                app.add_file(&f);
            }
        }
        app.rebuild();
        app
    }

    fn open_device(&mut self, eng: engine::Engine) -> Audio {
        let Some(p) = &self.profile else {
            return Audio::Silent(
                audio::start_silent(self.rate, 256, eng),
                "No audio device found".into(),
            );
        };
        // `start` consumes the engine even when it fails, so try with a
        // stand-in first only if needed: open directly, and fall back to a
        // fresh engine pair running silently.
        let mix = p.mix_buffer;
        let started = match mix {
            Some(b) => audio::start(p, audio::mix_ahead(eng, b)),
            None => audio::start(p, eng),
        };
        match started {
            Ok(r) => Audio::Running(r),
            Err(e) => {
                let eng = self.replace_engine();
                let silent = match mix {
                    Some(b) => audio::start_silent(self.rate, 256, audio::mix_ahead(eng, b)),
                    None => audio::start_silent(self.rate, 256, eng),
                };
                Audio::Silent(silent, e)
            }
        }
    }

    /// Starts a fresh engine on the chosen device, keeping the session.
    fn restart_audio(&mut self) {
        // Stop the old audio before starting the new.
        self.audio = Audio::Stopped;
        self.rate = self.profile.as_ref().map_or(self.rate, |p| p.rate);
        // Files and takes at the old rate are opened again at the new one.
        let mut stale: Vec<PathBuf> = (self.tracks.iter().flat_map(|t| &t.clips))
            .filter(|c| c.source.rate != self.rate)
            .map(|c| c.path.clone())
            .collect();
        stale.sort();
        stale.dedup();
        for path in stale {
            self.start_import(path, true, 0);
        }
        let eng = self.replace_engine();
        self.audio = self.open_device(eng);
        if self.ctl.wants_twin() {
            // Split once started: send the graph again with the live
            // tracks' copy.
            self.rebuild();
        }
    }

    /// Turns the big mix buffer on (at the size last used, or 2048) or
    /// off, saving it with the device's settings, and restarts the audio.
    pub(crate) fn toggle_mix_ahead(&mut self) {
        let Some(p) = &mut self.profile else {
            self.message = Some("Choose an audio device first.".into());
            return;
        };
        p.mix_buffer = match p.mix_buffer {
            Some(_) => None,
            None => Some(self.last_mix_buffer.unwrap_or(2048)),
        };
        if let Some(b) = p.mix_buffer {
            self.last_mix_buffer = Some(b);
        }
        let p = p.clone();
        if let Some(d) = &mut self.draft
            && (&d.host, &d.device) == (&p.host, &p.device)
        {
            d.mix_buffer = p.mix_buffer;
        }
        self.saved.use_profile(p);
        if let Err(e) = save_settings(&self.saved) {
            self.message = Some(format!("Could not save the device settings: {e}"));
        }
        self.restart_audio();
    }

    /// A fresh engine with this session, metronome, loop and position;
    /// the returned half goes to the audio device. Recording stops, which
    /// finishes any take in progress.
    fn replace_engine(&mut self) -> engine::Engine {
        let pos = self.ctl.status().position();
        let (ctl, eng, disk, recorder) = new_engine();
        let mut old = std::mem::replace(&mut self.ctl, ctl);
        self._disk = disk;
        // Dropping the old recorder finishes any take in progress; keep it.
        drop(std::mem::replace(&mut self._recorder, recorder));
        let takes = old.finished_takes();
        self.add_takes(takes);
        self.recording = false;
        // The old engine took its effects with it: make them again, at
        // the new rate, with the same settings.
        let rate = self.profile.as_ref().map_or(self.rate, |p| p.rate);
        self.effects.pending.clear();
        self.effects.pending_instruments.clear();
        self.pending_instruments.clear();
        for i in self.tracks.iter_mut().flat_map(|t| t.instruments_mut()) {
            if let Err(e) = i.remake(&mut self.effects, rate, &mut self.pending_instruments) {
                self.message = Some(format!("{}: {e}", i.name));
            }
        }
        self.midi.start(&mut self.ctl);
        for t in &mut self.tracks {
            let mut failed = Vec::new();
            for (i, slot) in t.fx.iter_mut().enumerate() {
                if let Err(e) = self.effects.reload(slot, rate) {
                    self.message = Some(format!("{}: {e}", slot.name));
                    failed.push(i);
                }
            }
            for i in failed.into_iter().rev() {
                t.fx.remove(i);
            }
        }
        self.rebuild();
        self.send(Command::SetMetronome(self.metronome));
        self.send_input_tap();
        self.send(self.meters.command());
        self.send(self.practice());
        self.send_loop();
        self.send(Command::Seek(pos));
        self.send_capture();
        eng
    }

    /// Notices a device that has stopped, plays silently meanwhile, and
    /// starts again on it once it is back.
    fn watch_device(&mut self, ctx: &egui::Context) {
        if let Audio::Running(r) = &self.audio
            && r.status.failed.load(Ordering::Relaxed)
        {
            self.restart_audio();
            if !matches!(self.audio, Audio::Running(_)) {
                self.message = Some(
                    "The audio device stopped (unplugged?). Playing silently until it is back."
                        .into(),
                );
                self.reconnect = Some(Instant::now() + RECONNECT_EVERY);
            }
        }
        let Some(at) = self.reconnect else { return };
        if Instant::now() >= at {
            if self.profile.as_ref().is_some_and(audio::is_present) {
                self.restart_audio();
            }
            if matches!(self.audio, Audio::Running(_)) {
                self.reconnect = None;
                self.message = None;
            } else {
                self.reconnect = Some(Instant::now() + RECONNECT_EVERY);
            }
        }
        ctx.request_repaint_after(RECONNECT_EVERY);
    }

    fn tempo_map(&self) -> TempoMap {
        tempo::map(self.rate, self.tempo_bpm, self.beats_per_bar, &self.time)
    }

    fn add_file(&mut self, path: &Path) {
        self.add_file_at(path, 0);
    }

    /// Adds the file as a new track, its clip starting at frame `at`.
    fn add_file_at(&mut self, path: &Path, at: i64) {
        // In full, so the project can find it from anywhere.
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        // A video's picture goes above the tracks; its sound comes in
        // as any audio does.
        if daw_video::is_video(&path) {
            self.video.add(&path, at);
        }
        self.start_import(path, false, at);
    }

    fn start_import(&mut self, path: PathBuf, reopen: bool, at: i64) {
        let (tx, rx) = mpsc::channel();
        let (p, rate) = (path.clone(), self.rate);
        std::thread::spawn(move || {
            let open = || {
                let p = if reopen {
                    p
                } else {
                    import::soundtrack(&p, &cache_dir())?
                };
                let s = import::open_at_rate(&p, rate, &cache_dir())?;
                Ok((p, s))
            };
            let _ = tx.send(open());
        });
        self.imports.push(Import {
            path,
            reopen,
            at,
            rate,
            done: rx,
        });
    }

    /// Takes in files that have finished opening. Returns true if any did.
    fn finish_imports(&mut self) -> bool {
        let mut changed = false;
        // New tracks are added in the order the files were given, so one
        // waits for any earlier one still converting.
        let mut waiting = false;
        let mut i = 0;
        while i < self.imports.len() {
            let new_track = !self.imports[i].reopen;
            if new_track && waiting {
                i += 1;
                continue;
            }
            let result = match self.imports[i].done.try_recv() {
                Ok(r) => r,
                Err(mpsc::TryRecvError::Empty) => {
                    waiting |= new_track;
                    i += 1;
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => Err("the import stopped".into()),
            };
            let im = self.imports.remove(i);
            match result {
                // The device changed rate while this was converting: start
                // again at the new one.
                Ok((path, _)) if im.rate != self.rate => self.start_import(path, im.reopen, im.at),
                Ok((path, source)) => {
                    changed = true;
                    if im.reopen {
                        // A new device rate, not an edit.
                        self.proj.quiet_next = true;
                        for c in self.tracks.iter_mut().flat_map(|t| &mut t.clips) {
                            if c.path == im.path && c.source.rate != source.rate {
                                c.reopened(source.clone());
                            }
                        }
                    } else {
                        let mut t = Track::new(
                            path.file_stem()
                                .map_or("Track".into(), |s| s.to_string_lossy().into_owned()),
                        );
                        let c = Clip::new(source, path, im.at.max(0), None);
                        self.video.sound_added(&im.path, &c.key);
                        t.clips.push(c);
                        self.tracks.push(t);
                    }
                }
                // A video with no sound we can read still has its
                // picture.
                Err(_) if self.video.has(&im.path) => {}
                Err(e) => self.message = Some(e),
            }
        }
        changed
    }

    /// Keeps clips that follow tempo in step with the tempo map: after
    /// a tempo change they move to where their beat now falls, after
    /// an edit their beat follows where they are; and works out how
    /// long every stretched clip is on the timeline.
    fn sync_clips(&mut self) {
        let map = self.tempo_map();
        let retime = std::mem::take(&mut self.retime);
        for t in &mut self.tracks {
            for c in &mut t.clips {
                if c.tempo.is_some() {
                    // A clip already inside its beat's tick stays where it
                    // is, so reloading the project doesn't pull clips
                    // placed between ticks onto the tick grid.
                    if retime && map.ticks_at(Frames(c.start)).0 != c.beat {
                        c.start = map.frames_at(daw_engine::time::Ticks(c.beat)).0;
                    } else if !retime {
                        c.beat = map.ticks_at(Frames(c.start)).0;
                    }
                }
                c.retime(&map);
            }
        }
    }

    /// Compiles the session and sends it to the engine.
    fn rebuild(&mut self) {
        self.sync_clips();
        self.fill_patterns();
        let s = self.session();
        self.pending_instruments
            .append(&mut self.effects.pending_instruments);
        match s.compile_all(
            audio::MAX_BLOCK,
            &mut self.effects.pending,
            &mut self.pending_instruments,
        ) {
            Ok(g) => {
                // Split, the live tracks play from a copy of their own.
                let g = match self.ctl.wants_twin() {
                    true => match s.compile(audio::MAX_BLOCK) {
                        Ok(twin) => g.with_twin(twin),
                        Err(_) => g,
                    },
                    false => g,
                };
                self.send_graph(g);
            }
            Err(e) => self.message = Some(e.to_string()),
        }
        self.send_monitor();
    }

    /// The session as the engine should play it now.
    fn session(&self) -> Session {
        let mut s = Session::new(self.tempo_map());
        s.groups = (self.groups.iter())
            .map(|g| daw_engine::graph::GroupSpec {
                gain_db: g.gain_db as f32,
                mute: g.mute,
            })
            .collect();
        for t in &self.tracks {
            if let Some(clip) = self.frozen_playing(t) {
                // Its clips and effects as rendered where it has every
                // plugin.
                let mut spec = TrackSpec::new(vec![clip]);
                spec.mix = t.heard();
                spec.input = t.input;
                spec.arm = t.arm;
                spec.monitor =
                    t.monitor && !self.profile.as_ref().is_some_and(|p| p.direct_monitoring);
                // Its plugins that are here stay in the graph, bypassed,
                // for when the frozen audio stops fitting.
                spec.inserts = fx::specs(&t.fx)
                    .into_iter()
                    .map(|f| daw_engine::graph::InsertSpec { bypass: true, ..f })
                    .collect();
                self.route(t, &mut spec);
                spec.automation = self.lane_specs(t);
                s.tracks.push(spec);
                continue;
            }
            // A file still being converted to a new rate plays nothing
            // until it is ready, but keeps its place.
            let clips = (t.clips.iter())
                .filter(|c| !c.muted && c.source.rate == self.rate)
                .map(|c| clip_spec(c, &s.tempo))
                .collect();
            let mut spec = TrackSpec::new(clips);
            spec.mix = t.heard();
            spec.input = t.input;
            spec.arm = t.arm;
            // With direct monitoring the interface plays the input.
            spec.monitor = t.monitor && !self.profile.as_ref().is_some_and(|p| p.direct_monitoring);
            spec.inserts = fx::specs(&t.fx);
            layers::place(&t.layers, &t.fx, &mut spec.inserts);
            let (notes, expression, controls) = t.note_specs();
            spec.notes = notes.into();
            spec.expression = expression.into();
            spec.controls = controls;
            spec.instrument = t.instrument.as_ref().map(instrument::Slot::spec);
            spec.layers = t.layers.iter().map(layers::Layer::spec).collect();
            spec.notes_live = t.kind.plays_notes() && (t.arm || t.monitor);
            spec.note_repeat = t.note_repeat.map(|r| daw_engine::time::Ticks(r.ticks()));
            if t.kind.plays_notes() {
                spec.note_fx = notefx::specs(&t.note_fx);
                spec.note_scale = notefx::scale_spec(self.scale);
            }
            spec.midi_out = t.midi_out.filter(|_| t.kind.plays_notes());
            self.route(t, &mut spec);
            spec.automation = self.lane_specs(t);
            s.tracks.push(spec);
        }
        s.master_db = self.master.mix.gain_db;
        s.master_inserts = fx::specs(&self.master.fx);
        s.master_automation = self.lane_specs(&self.master);
        s
    }

    /// Makes fresh copies of the master's effects for an offline render
    /// of `s`, as `fresh` does for a track's, where `s` keeps them. The
    /// copies add no helper process's block of delay.
    fn fresh_master_fx(
        &mut self,
        s: &mut Session,
        plugins: &mut Vec<daw_plugins::Plugin>,
        fx: &mut Vec<(u64, Box<dyn daw_engine::graph::Insert>)>,
    ) -> Result<(), String> {
        fresh_master_fx(
            &mut self.effects,
            &self.master.fx,
            self.rate,
            s,
            plugins,
            fx,
        )
    }

    /// The index of the track with this key.
    fn track_index(&self, key: &str) -> Option<usize> {
        self.tracks.iter().position(|t| t.key == key)
    }

    /// A bus the track can be routed to: a bus or folder that is there.
    fn bus_index(&self, key: &str) -> Option<usize> {
        self.track_index(key)
            .filter(|&i| self.tracks[i].kind.is_bus())
    }

    /// Fills in where `t`'s audio goes, by index: its output, sends,
    /// control group and effects' sidechains. Anything naming a track
    /// that isn't there goes to the master, or is left out.
    fn route(&self, t: &Track, spec: &mut TrackSpec) {
        spec.output = match t.goes_to().and_then(|k| self.bus_index(k)) {
            Some(i) if self.tracks[i].key != t.key => Output::Track(i),
            _ => Output::Master,
        };
        spec.sends = (self.sends_of(t).iter())
            .map(|(s, to)| daw_engine::graph::SendSpec {
                to: *to,
                gain_db: s.gain_db as f32,
                pre_fader: s.pre_fader,
                mute: s.mute,
            })
            .collect();
        spec.group = (t.group.as_deref()).and_then(|g| self.groups.iter().position(|x| x.id == g));
        for (slot, spec) in t.fx.iter().zip(&mut spec.inserts) {
            spec.sidechain = (slot.sidechain.as_deref())
                .and_then(|k| self.track_index(k))
                .filter(|&i| self.tracks[i].key != t.key);
        }
        spec.cues = self.cue_sends(&t.key);
        spec.instrument_outputs = (t.instrument_outputs.iter())
            .filter_map(|o| {
                let to = self.bus_index(&o.to)?;
                (self.tracks[to].key != t.key).then_some(daw_engine::graph::AuxSpec {
                    output: o.output,
                    to,
                })
            })
            .collect();
    }

    /// The track's sends that go somewhere, with the bus's index: the
    /// order the engine has them in.
    fn sends_of<'a>(&self, t: &'a Track) -> Vec<(&'a daw_project::Send, usize)> {
        (t.sends.iter())
            .filter_map(|s| Some((s, self.bus_index(&s.to)?)))
            .filter(|(_, i)| self.tracks[*i].key != t.key)
            .collect()
    }

    /// Renders the mix (and stems, if asked) to a file in the background.
    pub(crate) fn start_export(&mut self, req: export::Request) -> Result<(), String> {
        let map = self.tempo_map();
        let mut s = self.session();
        // The mix as it stands: no practice mute (unless it is to play
        // along with), no live input.
        for (spec, t) in s.tracks.iter_mut().zip(&self.tracks) {
            spec.mix = if req.play_along { t.heard() } else { t.mix };
            spec.input = None;
            spec.arm = false;
            spec.monitor = false;
        }
        let mut plugins = Vec::new();
        let rate = self.rate;
        let mut graph = |s: &Session, tracks: &[&Track], plugins: &mut Vec<_>| {
            // The copies made for rendering add no helper process's
            // block of delay: what they report is what is made up for.
            let mut s = s.clone();
            let mut delays = std::collections::HashMap::new();
            let mut fx = Vec::new();
            for slot in tracks.iter().flat_map(|t| &t.fx) {
                let (p, proc) = self.effects.fresh(slot, rate)?;
                if let Some(p) = &p {
                    delays.insert(slot.id, p.latency());
                }
                plugins.extend(p);
                fx.push((slot.id, proc));
            }
            let mut instruments: Vec<(u64, Box<dyn daw_engine::midi::Instrument>)> = Vec::new();
            let mut inst_delays = std::collections::HashMap::new();
            for i in tracks.iter().flat_map(|t| t.instruments()) {
                if let Some((p, inst)) = i.fresh(&mut self.effects, rate)? {
                    if let Some(p) = &p {
                        inst_delays.insert(i.id, p.latency());
                    }
                    plugins.extend(p);
                    instruments.push((i.id, inst));
                }
            }
            for t in &mut s.tracks {
                for f in &mut t.inserts {
                    if let Some(&d) = delays.get(&f.id) {
                        f.latency = d;
                    }
                }
                let layers = t.layers.iter_mut().filter_map(|l| l.instrument.as_mut());
                for i in t.instrument.iter_mut().chain(layers) {
                    if let Some(&d) = inst_delays.get(&i.id) {
                        i.latency = d;
                    }
                }
            }
            fresh_master_fx(
                &mut self.effects,
                &self.master.fx,
                rate,
                &mut s,
                plugins,
                &mut fx,
            )?;
            s.compile_all(audio::MAX_BLOCK, &mut fx, &mut instruments)
                .map_err(|e| e.to_string())
        };
        let all: Vec<&Track> = self.tracks.iter().collect();
        let mix = graph(&s, &all, &mut plugins)?;
        let at_bar = |b: i64| map.frames_at(map.bar_start(b.max(1)));
        let (from, to) = if let Some((a, b)) = req.bars {
            (at_bar(a), at_bar(b))
        } else if req.loop_only {
            (
                map.frames_at(map.bar_start(self.loop_bars.0)),
                map.frames_at(map.bar_start(self.loop_bars.1)),
            )
        } else {
            // Room for effects to ring out after the last clip.
            let tail = if (self.tracks.iter())
                .any(|t| !t.fx.is_empty() || t.instruments().next().is_some())
            {
                2 * self.rate as i64
            } else {
                0
            };
            (Frames(0), Frames(mix.end().0 + tail))
        };
        let frames = (to.0 - from.0).max(0) as usize;
        let wanted = |t: &Track| req.only.as_ref().is_none_or(|o| o.contains(&t.key));
        let mut parts = Vec::new();
        // A practice file per section: cut at its bars, with no tail,
        // so it loops cleanly.
        for (i, sec) in req.sections.iter().enumerate() {
            let (a, b) = (at_bar(sec.from), at_bar(sec.to));
            parts.push(export::Part {
                name: format!("{:02} {}", i + 1, export::file_name(&sec.name)),
                graph: graph(&s, &all, &mut plugins)?,
                from: a,
                frames: (b.0 - a.0).max(1) as usize,
                film: None,
            });
        }
        if req.video {
            // The whole film from its first frame: the picture can't be
            // cut without re-encoding it.
            let at = self.ctl.status().position().0;
            let c = (self.video.current(at)).ok_or("There's no video to put the mix into.")?;
            parts.push(export::Part {
                name: format!("{} with the mix", export::file_name(&c.name)),
                graph: graph(&s, &all, &mut plugins)?,
                from: Frames(c.zero().max(0)),
                frames: (c.offset + c.len).max(1) as usize,
                film: Some(c.path.clone()),
            });
        }
        if req.mix {
            if frames == 0 {
                return Err("There is nothing to export yet.".into());
            }
            parts.insert(
                0,
                export::Part {
                    name: "Mix".into(),
                    graph: mix,
                    from,
                    frames,
                    film: None,
                },
            );
        }
        if req.stems {
            // A bus on its own has nothing going into it; buses have
            // stems of their own below.
            for (i, t) in self
                .tracks
                .iter()
                .enumerate()
                .filter(|(_, t)| !t.kind.is_bus() && wanted(t))
            {
                let mut one = Session::new(map.clone());
                let mut spec = s.tracks[i].clone();
                spec.mix.mute = false;
                spec.mix.solo = false;
                // On its own: no bus, sends, group or sidechains, but
                // through the master, as in the mix.
                mixer::alone(&mut spec);
                one.tracks.push(spec);
                one.master_db = s.master_db;
                one.master_inserts = s.master_inserts.clone();
                one.master_automation = s.master_automation.clone();
                parts.push(export::Part {
                    name: format!("{:02} {}", i + 1, export::file_name(&t.name)),
                    graph: graph(&one, &[t], &mut plugins)?,
                    from,
                    frames,
                    film: None,
                });
            }
        }
        if req.bus_stems {
            for (i, t) in self
                .tracks
                .iter()
                .enumerate()
                .filter(|(_, t)| t.kind.is_bus() && wanted(t))
            {
                let one = mixer::through_bus(&s, i);
                parts.push(export::Part {
                    name: format!("{:02} {}", i + 1, export::file_name(&t.name)),
                    graph: graph(&one, &all, &mut plugins)?,
                    from,
                    frames,
                    film: None,
                });
            }
        }
        if parts.is_empty() && req.package.as_ref().is_none_or(|p| p.files.is_empty()) {
            return Err("There is nothing to export yet.".into());
        }
        let word = if req.package.is_some() {
            "Stems"
        } else if req.sections.is_empty() {
            "Mix"
        } else {
            "Sections"
        };
        self.export
            .start(&req.folder, word, parts, plugins, req.format, req.package)
    }

    /// Starts `req` now, or queues it if an export is running. Any loop
    /// is fixed to its bars now, so a queued export keeps them.
    pub(crate) fn export_or_queue(&mut self, mut req: export::Request) -> Result<(), String> {
        if req.loop_only && req.bars.is_none() {
            req.bars = Some(self.loop_bars);
        }
        if self.export.busy() || self.export.queue().next().is_some() {
            self.export.enqueue(req);
            Ok(())
        } else {
            self.start_export(req)
        }
    }

    /// Starts the next queued export once the one running finishes.
    fn run_export_queue(&mut self) {
        if let Some(q) = self.export.next_queued()
            && let Err(e) = self.start_export(q.request)
        {
            self.export.failed(q.label, e);
        }
    }

    /// Exports the video at the playhead (or the one selected) with the
    /// mix as its sound, into a new folder beside the other exports, and
    /// shows the Export window for its progress.
    pub(crate) fn export_video(&mut self) {
        if self.export.busy() {
            return;
        }
        let req = self.export.video_request();
        match self.start_export(req) {
            Ok(()) => self.export.open = true,
            Err(e) => self.message = Some(e),
        }
    }

    fn send_graph(&mut self, g: daw_engine::graph::Graph) {
        if self.ctl.set_graph(g).is_err() {
            self.message = Some("The audio engine is busy; try again.".into());
        }
    }

    fn send(&mut self, cmd: Command) {
        if self.ctl.send(cmd).is_err() {
            self.message = Some("The audio engine is busy; try again.".into());
        }
    }

    fn send_loop(&mut self) {
        let map = self.tempo_map();
        let l = self.looping.then(|| {
            (
                map.frames_at(map.bar_start(self.loop_bars.0)),
                map.frames_at(map.bar_start(self.loop_bars.1)),
            )
        });
        self.send(Command::SetLoop(l));
    }

    /// Adds finished takes to their tracks. Returns true if any arrived.
    fn finish_takes(&mut self) -> bool {
        let takes = self.ctl.finished_takes();
        self.add_takes(takes)
    }

    fn add_takes(&mut self, takes: Vec<Take>) -> bool {
        let offset = match (&self.audio, &self.profile) {
            (Audio::Running(r), Some(p)) => r.record_offset(p),
            _ => 0,
        };
        let mut any = false;
        let mut keys = Vec::new();
        for take in takes {
            if let Some(e) = &take.error {
                self.message = Some(format!("Recording problem: {e}"));
            }
            if take.frames == 0 {
                continue;
            }
            match self.place_take(&take, offset) {
                Ok(k) => {
                    any = true;
                    keys.push(k);
                }
                Err(e) => self.message = Some(e),
            }
        }
        // For "undo last take" from a phone.
        self.remote.note_takes(keys);
        any
    }

    /// Returns the track's and the new clip's keys.
    fn place_take(&mut self, take: &Take, offset: u32) -> Result<(String, String), String> {
        let source = StreamSource::open(&take.path)?;
        let t = self
            .tracks
            .get_mut(take.track)
            .ok_or("a take arrived for a track that is gone")?;
        let start = take.clip_start(Frames(offset as i64)).0;
        let clip = Clip::new(source, take.path.clone(), start, None);
        let keys = (t.key.clone(), clip.key.clone());
        t.takes(self.rate).add_take(clip);
        Ok(keys)
    }

    /// The performance check's figures, and the test tracks once
    /// written.
    fn perf_window(&mut self, ctx: &egui::Context) {
        let action = self.perf.poll();
        self.perf_action(action);
        let status = match &self.audio {
            Audio::Running(r) | Audio::Silent(r, _) => r.status.clone(),
            Audio::Stopped => return,
        };
        self.perf.update(&status);
        if self.perf.open {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        let frames = status.last_frames.load(Ordering::Relaxed);
        if frames > 0 {
            // Helper processes for new effects run one such block behind.
            self.effects.block = frames as usize;
        }
    }

    /// Settings → Performance.
    pub(crate) fn perf_page(&mut self, ui: &mut egui::Ui) {
        let status = match &self.audio {
            Audio::Running(r) | Audio::Silent(r, _) => r.status.clone(),
            Audio::Stopped => {
                ui.label("The audio isn't running: choose a device in Settings → Audio.");
                return;
            }
        };
        let overloads = status.overloads.load(Ordering::Relaxed);
        let underruns = self.ctl.status().underruns();
        let frames = status.last_frames.load(Ordering::Relaxed);
        let buffer = (frames > 0).then_some((frames, self.rate));
        let effects: Vec<_> = (self.effects.list.plugins().into_iter())
            .filter(|p| p.enabled && !p.class.is_instrument())
            .collect();
        let now = perf::Now {
            overloads,
            underruns,
            buffer,
            tracks: self.tracks.len(),
            rate: self.rate,
        };
        let action = self.perf.page(ui, now, &effects);
        if let Some(perf::Action::Reset) = action {
            self.perf.reset(overloads, underruns);
        } else {
            self.perf_action(action);
        }
    }

    fn perf_action(&mut self, action: Option<perf::Action>) {
        match action {
            Some(perf::Action::AddTracks(files)) => {
                for f in files {
                    self.add_file(&f);
                }
            }
            Some(perf::Action::AddPlugin(a)) => {
                for i in 0..perf::PLUGINS.min(self.tracks.len()) {
                    match self.effects.create(&a, self.rate) {
                        Ok(slot) => self.tracks[i].fx.push(slot),
                        Err(e) => {
                            self.message = Some(format!("{}: {e}", a.class.name));
                            break;
                        }
                    }
                }
                self.rebuild();
            }
            Some(perf::Action::Reset) | None => {}
        }
    }

    /// Lesson notes and tab blocks (roadmap 1.7).
    fn notes_pane(&mut self, ui: &mut egui::Ui) {
        let map = self.tempo_map();
        let end_bar = self.with_song(|_, song| timeline::Timeline::end_bar(&map, song));
        let playhead = self.ctl.status().position();
        let me = self.my_author_id();
        let cx = notes::Ctx {
            markers: &self.markers,
            end_bar,
            playhead_bar: map.bar_beat(map.ticks_at(playhead)).bar,
            me: &me,
            may_edit: self.may_edit_song(),
        };
        let out = self.notes_ui.pane(ui, &mut self.notes, &mut self.tabs, &cx);
        if let Some(bar) = out.go_to_bar {
            let f = map.frames_at(map.bar_start(bar));
            self.send(Command::Seek(f));
        }
        if let Some(bars) = out.loop_bars {
            self.loop_bars = bars;
            self.looping = true;
            self.send_loop();
        }
    }

    /// Whether this user may change the song, not only comment on it.
    /// While suggesting edits, anyone may: they go to the suggestion.
    fn may_edit_song(&self) -> bool {
        self.proj.suggesting.is_some()
            || self.proj.doc.my_role() != Some(daw_project::Role::Commenter)
    }

    /// Who this user is, for the comments they write: this computer's id
    /// until projects have members, when it becomes the member's key.
    pub(crate) fn my_author_id(&self) -> String {
        if cfg!(test) {
            "me".into()
        } else {
            self.proj.doc.my_key().unwrap_or_default().to_string()
        }
    }

    /// Whether this user may edit or delete anyone's comment.
    /// Commenters, and people not let in yet, change only their own.
    pub(crate) fn may_change_any_comment(&self) -> bool {
        match self.proj.doc.my_role() {
            Some(r) => r != daw_project::Role::Commenter,
            None => self.proj.doc.members().is_empty(),
        }
    }

    /// The comments panel, at the side.
    fn comments_panel(&mut self, ui: &mut egui::Ui) {
        let map = self.tempo_map();
        let tracks: Vec<(String, String)> = (self.tracks.iter())
            .map(|t| (t.key.clone(), t.name.clone()))
            .collect();
        let at = |bar: i64| map.frames_at(map.bar_start(bar)).0;
        let me = self.my_author_id();
        let members = self.proj.doc.members();
        let any = self.may_change_any_comment();
        let cx = comments::Ctx {
            map: &map,
            playhead: self.ctl.status().position().0,
            loop_frames: self
                .looping
                .then(|| (at(self.loop_bars.0), at(self.loop_bars.1))),
            tracks: &tracks,
            me: &me,
            members: &members,
            may_change_any: any,
        };
        let out = self.comment_ui.panel(ui, &mut self.comments, &cx);
        if let Some(f) = out.play_from {
            self.send(Command::Seek(Frames(f)));
            if !self.ctl.status().playing() {
                self.toggle_play();
            }
        }
    }

    /// Runs `f` on the timeline and the song it edits.
    fn with_song<R>(&mut self, f: impl FnOnce(&mut timeline::Timeline, &mut Song) -> R) -> R {
        let mut song = Song {
            lanes: (self.tracks.iter_mut())
                .map(|t| Lane {
                    name: &t.name,
                    clips: &mut t.clips,
                    notes: &mut t.note_clips,
                    kind: t.kind,
                    in_folder: t.parent.is_some(),
                })
                .collect(),
            markers: &mut self.markers,
        };
        f(&mut self.timeline, &mut song)
    }

    /// The timeline, with its outcome acted on.
    fn timeline(&mut self, ui: &mut egui::Ui) {
        let map = self.tempo_map();
        let ids: Vec<String> = self.tracks.iter().map(|t| t.key.clone()).collect();
        let pins = self.comment_ui.pins(&self.comments, &ids);
        let commands = std::mem::take(&mut self.commands.timeline);
        let hits = self.hit_points.clone();
        let freeze: Vec<(bool, bool)> = (self.tracks.iter())
            .map(|t| (render::can_freeze(t), t.freeze))
            .collect();
        let versions = self.lane_versions();
        let heads = self.heads();
        let inspected = self.inspected();
        let view = timeline::View {
            map: &map,
            rate: self.rate,
            playhead: self.ctl.status().position().0,
            playing: self.ctl.status().playing(),
            loop_bars: self.looping.then_some(self.loop_bars),
            can_undo: self.can_undo(),
            can_redo: self.can_redo(),
            pins: &pins,
            commands: &commands,
            timecode: self.shown_clock(),
            hits: &hits,
            freeze: &freeze,
            versions: &versions,
            can_paste: !self.renders.copied.is_empty(),
            bands: &[],
            heads: &heads,
            inspected,
        };
        self.timeline.groups = self.lane_groups();
        self.timeline.simple = self.simple.on;
        self.timeline.band = if self.video.clips.is_empty() && !self.video.busy() {
            0.0
        } else {
            video::BAND
        };
        // The notation bands borrow their layouts, taken out of the app
        // while the timeline draws.
        self.refresh_notation(&map);
        let built = self.take_notation_layouts();
        let bands = App::bands(&self.tracks, &built, self.notation.entry.as_ref());
        let shown = timeline::View {
            bands: &bands,
            ..view
        };
        let out = self.with_song(|tl, song| tl.show(ui, song, &shown));
        drop(bands);
        self.put_notation_layouts(built);
        if let Some(r) = out.band {
            let snap = self.timeline.snap.then_some(self.timeline.grid);
            let band = self.video.band(
                ui,
                r,
                timeline::HEADER,
                self.timeline.x_view(),
                self.rate,
                view.playhead,
                &map,
                snap,
            );
            if let Some((key, by)) = band.moved
                && let Some(c) = (self.tracks.iter_mut())
                    .flat_map(|t| t.clips.iter_mut())
                    .find(|c| c.key == key)
            {
                c.start = (c.start + by).max(0);
                self.rebuild();
            }
            if let Some(moved) = band.picture {
                self.picture_moved(moved);
            }
            if let Some(f) = band.seek {
                self.send(Command::Seek(Frames(f)));
            }
        }
        if let Some((lane, tick, y)) = out.notation_band {
            self.band_clicked(lane, tick, y);
        } else if out.off_band {
            self.notation.entry = None;
        }
        if let Some(id) = out.pin {
            self.comment_ui.show_thread(&id);
        }
        if let Some(marker) = out.tab_for {
            if self.may_edit_song() {
                let me = self.my_author_id();
                self.notes_ui.add_tab(&mut self.tabs, Some(marker), &me);
            } else {
                self.notes_ui.open = true;
            }
        }
        if let Some((lane, at)) = out.comment_at {
            self.comment_ui.write_at(comments::Target {
                track: lane.and_then(|l| self.tracks.get(l)).map(|t| t.key.clone()),
                start: at,
                end: None,
            });
        }
        if let Some(key) = out.edit_notes {
            self.notes_editor.edit(&key);
            self.open_pane(workspace::Pane::NotesEditor);
        }
        if out.edit_audio {
            self.open_pane(workspace::Pane::PitchEditor);
        }
        match out.pattern {
            Some(timeline::PatternAsk::Add(lane, at)) => {
                self.add_pattern_clip(lane, at);
            }
            Some(timeline::PatternAsk::Edit(key)) => self.edit_pattern_of(&key),
            // Through the commands, so they act the same from the menu,
            // the keys and outside the app.
            Some(timeline::PatternAsk::Unique(key)) => {
                self.timeline.note_sel = Some(key);
                self.run(ui.ctx(), commands::Cmd::MakePatternUnique);
            }
            Some(timeline::PatternAsk::ToNotes(key)) => {
                self.timeline.note_sel = Some(key);
                self.run(ui.ctx(), commands::Cmd::PatternToNotes);
            }
            None => {}
        }
        if let Some(how) = out.process {
            self.processing
                .start(&self.tracks, &self.timeline.selected, how);
        }
        if out.copy {
            self.copy_clips();
        }
        if out.bounce {
            self.bounce_selection();
        }
        if let Some((lane, at, rendered)) = out.paste {
            if rendered {
                self.paste_rendered(lane, at);
            } else if self.paste_clips(Some(lane), at) {
                self.rebuild();
            }
        }
        if let Some((i, ask)) = out.head {
            self.header_asked(i, ask);
        }
        match out.track {
            Some((i, timeline::TrackAsk::Instrument)) => self.instrument_open = Some(i),
            Some((i, timeline::TrackAsk::Effects)) => self.effects.open_track = Some(i),
            Some((i, timeline::TrackAsk::Freeze)) => self.toggle_freeze(ui.ctx(), i),
            Some((i, timeline::TrackAsk::RenderInPlace)) => self.render_in_place(i),
            Some((i, timeline::TrackAsk::SavePreset)) => {
                self.ask_save_preset(Some(i), daw_project::PresetKind::Track)
            }
            Some((i, timeline::TrackAsk::Version(ask))) => self.ask_version(ui.ctx(), i, ask),
            None => {}
        }
        if let Some(cmd) = out.command {
            self.run(ui.ctx(), cmd);
        }
        if out.undo {
            self.undo(ui.ctx());
        }
        if out.redo {
            self.redo(ui.ctx());
        }
        if out.changed {
            self.rebuild();
        }
        if let Some(f) = out.seek {
            self.send(Command::Seek(Frames(f)));
        }
        if let Some(bars) = out.loop_bars {
            self.loop_bars = bars;
            self.looping = true;
            self.send_loop();
        }
        if let Some(ask) = out.tempo {
            self.ask_tempo(ask);
        }
        if let Some(ask) = out.hit {
            self.ask_hit(ask);
        }
    }

    fn toggle_play(&mut self) {
        if self.ctl.status().playing() {
            self.send(Command::Stop);
            return;
        }
        self.prepare_play();
        if self.count_in {
            self.send(Command::CountIn { bars: 1 });
        } else {
            self.send(Command::Play);
        }
    }

    pub(crate) fn practice(&self) -> Command {
        Command::SetPractice {
            speed: self.speed_pct / 100.0,
            semitones: self.semitones as f64 + self.cents as f64 / 100.0,
        }
    }

    /// The device's input channels.
    fn input_count(&self) -> usize {
        match &self.audio {
            Audio::Running(r) | Audio::Silent(r, _) => r.input_channels as usize,
            Audio::Stopped => 0,
        }
    }

    /// Settings → Audio.
    pub(crate) fn device_page(&mut self, ui: &mut egui::Ui) {
        let mut apply = false;
        let mut guide = false;
        {
            if self.hosts.iter().all(|h| h.devices.is_empty()) {
                ui.label("No audio devices were found.");
            }
            let draft = self.draft.get_or_insert_with(|| DeviceProfile {
                host: self.hosts.first().map_or(String::new(), |h| h.name.clone()),
                device: String::new(),
                rate: 48_000,
                buffer: None,
                main: (0, 1),
                click: None,
                input: None,
                record_offset: None,
                measured_at: None,
                direct_monitoring: false,
                monitor: daw_audio_io::MonitorOutputs::default(),
                mix_buffer: None,
            });
            egui::Grid::new("device").num_columns(2).show(ui, |ui| {
                    ui.label("Driver");
                    egui::ComboBox::from_id_salt("host")
                        .selected_text(&draft.host)
                        .show_ui(ui, |ui| {
                            for h in &self.hosts {
                                // ai: set_audio_device.driver
                                ui.selectable_value(&mut draft.host, h.name.clone(), &h.name);
                            }
                        })
.response
.accessible_name("Driver");
                    ui.end_row();

                    let host = self.hosts.iter().find(|h| h.name == draft.host);
                    ui.label("Device");
                    egui::ComboBox::from_id_salt("device")
                        .selected_text(&draft.device)
                        .width(260.0)
                        .show_ui(ui, |ui| {
                            for d in host.map(|h| h.devices.as_slice()).unwrap_or(&[]) {
                                // ai: set_audio_device.device
                                if ui
                                    .selectable_value(&mut draft.device, d.name.clone(), &d.name)
                                    .clicked()
                                    && let Some(p) = self.saved.for_device(&draft.host, &d.name)
                                {
                                    // Used before: start from its settings.
                                    *draft = p.clone();
                                }
                            }
                        })
.response
.accessible_name("Device");
                    ui.end_row();

                    let dev = host.and_then(|h| h.devices.iter().find(|d| d.name == draft.device));
                    ui.label("Sample rate");
                    egui::ComboBox::from_id_salt("rate")
                        .selected_text(format!("{} Hz", draft.rate))
                        .show_ui(ui, |ui| {
                            let rates = dev.map(|d| d.rates.clone()).unwrap_or_default();
                            for r in rates {
                                // ai: set_audio_device.rate
                                ui.selectable_value(&mut draft.rate, r, format!("{r} Hz"));
                            }
                        })
.response
.accessible_name("Sample rate");
                    ui.end_row();

                    ui.label("Buffer size");
                    egui::ComboBox::from_id_salt("buffer")
                        .selected_text(draft.buffer.map_or("Driver default".into(), |b| {
                            format!("{b} samples ({:.1} ms)", b as f64 * 1000.0 / draft.rate as f64)
                        }))
                        .show_ui(ui, |ui| {
                            // ai: set_audio_device.buffer
                            ui.selectable_value(&mut draft.buffer, None, "Driver default");
                            for b in [32, 64, 128, 256, 512, 1024, 2048] {
                                let ok = dev
                                    .and_then(|d| d.buffer_range)
                                    .is_none_or(|(lo, hi)| (lo..=hi).contains(&b));
                                // ai: set_audio_device.buffer
                                if ok {
                                    ui.selectable_value(&mut draft.buffer, Some(b), format!("{b} samples"));
                                }
                            }
                        })
.response
.accessible_name("Buffer size");
                    ui.end_row();

                    ui.label("Mix buffer").on_hover_text(MIX_BUFFER_HINT);
                    egui::ComboBox::from_id_salt("mix-buffer")
                        .selected_text(mix_buffer_text(draft.mix_buffer, draft.rate))
                        .show_ui(ui, |ui| {
                            // ai: mix-buffer
                            ui.selectable_value(&mut draft.mix_buffer, None, mix_buffer_text(None, draft.rate));
                            for b in daw_engine::ahead::MIX_BLOCKS {
                                ui.selectable_value(
                                    &mut draft.mix_buffer,
                                    Some(b),
                                    mix_buffer_text(Some(b), draft.rate),
                                );
                            }
                        })
                        .response
                        .on_hover_text(MIX_BUFFER_HINT)
                        .accessible_name("Mix buffer");
                    ui.end_row();

                    let outs = dev.map_or(2, |d| d.output_channels as usize).max(1);
                    ui.label("Main outputs");
                    ui.horizontal(|ui| {
                        channel_picker(ui, "main-l", "Main output, left", &mut draft.main.0, outs);
                        channel_picker(ui, "main-r", "Main output, right", &mut draft.main.1, outs);
                    });
                    ui.end_row();

                    ui.label("Input device");
                    egui::ComboBox::from_id_salt("input")
                        .selected_text(draft.input.as_deref().unwrap_or("None"))
                        .width(260.0)
                        .show_ui(ui, |ui| {
                            // ai: set_audio_device.input
                            ui.selectable_value(&mut draft.input, None, "None");
                            for d in &self.input_devices {
                                // ai: set_audio_device.input
                                ui.selectable_value(&mut draft.input, Some(d.clone()), d);
                            }
                        })
.response
.accessible_name("Input device");
                    ui.end_row();

                    ui.label("Recording offset");
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            let mut manual = draft.record_offset.is_some();
                            // ai: set_audio_device.record_offset
                            if ui.checkbox(&mut manual, "Measured").changed() {
                                draft.record_offset = manual.then_some(0);
                                draft.measured_at = None;
                            }
                            if let Some(o) = &mut draft.record_offset {
                                // ai: set_audio_device.record_offset
                                if ui
                                    .add(egui::DragValue::new(o).range(0..=48_000).suffix(" frames"))
                                    .changed()
                                {
                                    draft.measured_at = None;
                                }
                                ui.label(format!("{:.2} ms", *o as f64 * 1000.0 / draft.rate as f64));
                            } else {
                                ui.label("from the driver's reported latency");
                            }
                            if ui
                                // ai: measure-latency
                                .button("Measure…")
                                .on_hover_text("Measure the round trip with a loopback cable")
                                .clicked()
                            {
                                self.latency.open = true;
                                self.latency.inputs = input_channels(&draft.host, draft.input.as_deref());
                            }
                        });
                        if let Some((rate, buffer)) = draft.measured_at
                            && (rate, buffer) != (draft.rate, draft.buffer)
                        {
                            ui.colored_label(
                                ui.visuals().warn_fg_color,
                                format!(
                                    "Measured at {rate} Hz, {}: measure again for these settings.",
                                    buffer.map_or("driver default buffer".into(), |b| format!("{b} samples"))
                                ),
                            );
                        }
                    });
                    ui.end_row();

                    ui.label("Monitoring");
                    // ai: set_audio_device.direct_monitoring
                    ui.checkbox(&mut draft.direct_monitoring, "Direct: I listen through the interface")
                        .on_hover_text("The app never plays inputs back, so you don't hear them twice. Use when the interface's own monitor knob or switch is on.");
                    ui.end_row();

                    ui.label("Metronome output");
                    egui::ComboBox::from_id_salt("click")
                        .selected_text(draft.click.map_or("Main mix".into(), |c| format!("Output {}", c + 1)))
                        .show_ui(ui, |ui| {
                            // ai: set_audio_device.click_output
                            ui.selectable_value(&mut draft.click, None, "Main mix");
                            for c in 0..outs {
                                // ai: set_audio_device.click_output
                                ui.selectable_value(&mut draft.click, Some(c), format!("Output {}", c + 1));
                            }
                        })
.response
.accessible_name("Metronome output");
                    ui.end_row();
                });
            ui.separator();
            match &self.audio {
                Audio::Running(r) => {
                    let s = &r.status;
                    if s.failed.load(Ordering::Relaxed) {
                        ui.colored_label(
                            ui.visuals().error_fg_color,
                            "The device stopped with an error.",
                        );
                    } else {
                        ui.label(format!(
                            "Running: {} channels at {} Hz, {} samples per callback.",
                            r.channels,
                            r.rate,
                            s.last_frames.load(Ordering::Relaxed)
                        ));
                    }
                    let us = s.output_latency_us.load(Ordering::Relaxed);
                    let ins = s.input_latency_us.load(Ordering::Relaxed);
                    ui.label(format!(
                        "Latency reported by the driver: output {:.1} ms, input {:.1} ms.",
                        us as f64 / 1000.0,
                        ins as f64 / 1000.0
                    ));
                    match (&r.input_error, r.input_channels) {
                        (Some(e), _) => {
                            ui.colored_label(ui.visuals().error_fg_color, format!("No input: {e}"));
                        }
                        (None, 0) => {
                            ui.label("No input selected.");
                        }
                        (None, n) => {
                            ui.label(format!("{n} input channels."));
                        }
                    }
                    if let Some(p) = &self.profile {
                        ui.label(format!(
                            "Recordings are moved {} frames earlier to line up.",
                            r.record_offset(p)
                        ));
                    }
                    if let Some(line) = mix_ahead_line(self.ctl.status()) {
                        ui.label(line);
                    }
                }
                Audio::Stopped => {}
                Audio::Silent(_, why) => {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        format!("{why}. Playing silently, so the app still works."),
                    );
                }
            }
            ui.label(
                    RichText::new(
                        "For exact alignment, measure the round trip with a loopback cable: Recording offset → Measure….",
                    )
                    .small()
                    .weak(),
                );
            ui.horizontal(|ui| {
                // ai: set_audio_device
                if ui.button("Apply").clicked() {
                    apply = true;
                }
                if ui
                    // ai: audio-setup
                    .button("Guided setup…")
                    .on_hover_text("Step by step, with checks")
                    .clicked()
                {
                    guide = true;
                }
                // ai: set_audio_device.rescan
                if ui.button("Rescan devices").clicked() {
                    self.hosts = audio::list();
                    self.input_devices = input_names(&draft_host(&self.draft));
                }
            });
        }
        if apply {
            self.apply_draft();
        }
        if guide {
            self.run(&ui.ctx().clone(), Cmd::AudioSetup);
        }
    }

    /// Uses the settings in the device window, saves them and restarts
    /// the audio.
    fn apply_draft(&mut self) {
        self.reconnect = None;
        self.profile = self.draft.clone();
        if let Some(p) = &self.profile {
            self.saved.use_profile(p.clone());
            if let Err(e) = save_settings(&self.saved) {
                self.message = Some(format!("Could not save the device settings: {e}"));
            }
        }
        self.restart_audio();
    }

    /// What to measure latency on: the device in the device window, or,
    /// with `DAW_TEST_LOOPBACK` set, the silent stand-in's simulated
    /// cable.
    fn measure_target(&self) -> Result<audio::latency::Target, String> {
        let draft = self.draft.as_ref().ok_or("Choose an audio device first.")?;
        #[cfg(test)]
        let input = match self.loopback_for_tests {
            Some(delay) => audio::SilentInput::Loopback { delay },
            None => audio::SilentInput::from_env(),
        };
        #[cfg(not(test))]
        let input = audio::SilentInput::from_env();
        match input {
            input @ audio::SilentInput::Loopback { .. } => Ok(audio::latency::Target::Silent {
                rate: draft.rate,
                buffer: draft.buffer.unwrap_or(256),
                input,
            }),
            _ if draft.device.is_empty() => Err("Choose an audio device first.".into()),
            _ => Ok(audio::latency::Target::Device(Box::new(draft.clone()))),
        }
    }

    /// Stops the app's audio and starts measuring on `target`.
    fn start_measuring(&mut self, target: Result<audio::latency::Target, String>) {
        let target = match target {
            Ok(t) => t,
            Err(e) => {
                self.message = Some(e);
                return;
            }
        };
        self.reconnect = None;
        // Frees the device for the measurement.
        self.audio = Audio::Stopped;
        if !self.latency.begin(target) {
            self.restart_audio();
        }
    }

    fn latency_window(&mut self, ctx: &egui::Context) {
        if let Some(done) = self.latency.poll() {
            match (done, &mut self.draft) {
                (Ok(m), Some(d)) => {
                    d.record_offset = Some(m.frames);
                    d.measured_at = Some((d.rate, d.buffer));
                    self.apply_draft();
                }
                _ => self.restart_audio(),
            }
        }
        if self.latency.busy() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    /// Settings → Latency.
    pub(crate) fn latency_page(&mut self, ui: &mut egui::Ui) {
        let Some(draft) = self.draft.clone() else {
            ui.label("Choose an audio device first, in Settings → Audio.");
            return;
        };
        match self.latency.page(ui, &draft, self.recording) {
            Some(latency::Action::Start) => {
                let target = self.measure_target();
                self.start_measuring(target);
            }
            Some(latency::Action::Cancel) => {
                self.latency.cancel();
                self.restart_audio();
            }
            None => {}
        }
    }
}

/// Settings are kept per user: `%APPDATA%\<name>\settings.txt` on
/// Windows, `~/.config/<name>/settings.txt` elsewhere.
fn settings_path() -> PathBuf {
    config_dir().join("settings.txt")
}

/// Per-user settings folder: `%APPDATA%\<name>` on Windows,
/// `~/.config/<name>` elsewhere.
pub(crate) fn config_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_NAME)
}

fn load_settings() -> SavedProfiles {
    std::fs::read_to_string(settings_path())
        .map(|t| SavedProfiles::from_text(&t))
        .unwrap_or_default()
}

fn save_settings(s: &SavedProfiles) -> std::io::Result<()> {
    // Tests never touch the user's settings.
    if cfg!(test) {
        return Ok(());
    }
    let path = settings_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Written whole and renamed, so a crash never leaves half a file.
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, s.to_text())?;
    std::fs::rename(tmp, path)
}

const MIX_BUFFER_HINT: &str = "Armed and monitored tracks, and the buses they play through, \
    play at the buffer size above, so playing through an amp feels immediate. Everything else \
    plays ahead at this bigger buffer, so a heavy mix has time to run. Play, stop and other \
    changes reach your ears this much later.";

/// A mix buffer as the device settings show it.
fn mix_buffer_text(b: Option<u32>, rate: u32) -> String {
    match b {
        None => "Off: everything at the buffer size".into(),
        Some(b) => format!(
            "{b} samples ({:.0} ms)",
            b as f64 * 1000.0 / rate.max(1) as f64
        ),
    }
}

/// How the mix running ahead is doing, for the device settings and the
/// performance check.
pub(crate) fn mix_ahead_line(s: &engine::Status) -> Option<String> {
    let block = s.ahead.block()?;
    let late = s.ahead.late();
    let mut line = format!(
        "The rest of the mix plays ahead in blocks of {block} samples, using {:.0}% of the time it has.",
        s.ahead.load() * 100.0
    );
    if late > 0 {
        line += &format!(" It fell behind for {late} samples, so the backing paused.");
    }
    Some(line)
}

fn new_engine() -> (Controller, engine::Engine, StreamerThread, RecorderThread) {
    let (mut ctl, eng) = engine::new(audio::MAX_BLOCK);
    let disk = ctl.take_streamer().expect("new controller").spawn();
    let recorder = ctl.take_recorder().expect("new controller").spawn();
    ctl.set_record_dir(takes_dir());
    (ctl, eng, disk, recorder)
}

fn draft_host(p: &Option<DeviceProfile>) -> String {
    p.as_ref().map_or(String::new(), |p| p.host.clone())
}

/// Channels on the input device `name`, for choosing one; 2 if it can't
/// be found (so the simulated cable has some).
fn input_channels(host: &str, name: Option<&str>) -> usize {
    audio::input_devices(host)
        .into_iter()
        .find(|d| Some(d.name.as_str()) == name)
        .map_or(2, |d| d.input_channels as usize)
}

fn input_names(host: &str) -> Vec<String> {
    audio::input_devices(host)
        .into_iter()
        .map(|d| d.name)
        .collect()
}

/// Chooses a track's input: none, one channel, or a stereo pair.
/// Returns true if it changed.
fn input_picker(
    ui: &mut egui::Ui,
    track: usize,
    track_name: &str,
    input: &mut Option<Input>,
    inputs: usize,
) -> bool {
    let name = |i: &Option<Input>| match i {
        None => "None".to_string(),
        Some(i) if i.stereo => format!("In {}+{}", i.first + 1, i.first + 2),
        Some(i) => format!("In {}", i.first + 1),
    };
    let before = *input;
    egui::ComboBox::from_id_salt(("input", track))
        .width(70.0)
        .selected_text(name(input))
        .show_ui(ui, |ui| {
            // ai: set_track_input.input
            ui.selectable_value(input, None, "None");
            for c in 0..inputs {
                let i = Some(Input::mono(c));
                // ai: set_track_input.input
                ui.selectable_value(input, i, name(&i));
            }
            for c in (0..inputs.saturating_sub(1)).step_by(2) {
                let i = Some(Input::stereo(c));
                // ai: set_track_input.stereo
                ui.selectable_value(input, i, name(&i));
            }
        })
        .response
        .accessible_name(format!("Input for {track_name}"));
    *input != before
}

/// Chooses which take plays, newest first. Returns the choice if one
/// was made.
fn take_picker(ui: &mut egui::Ui, track: usize, t: &Track) -> Option<Option<usize>> {
    let n = timeline::take_count(&t.clips);
    if n == 0 {
        ui.label("");
        return None;
    }
    let now = timeline::active_take(&t.clips);
    let mut pick = now;
    let shown = match t.comp_name() {
        Some(comp) => comp.to_string(),
        None => now.map_or("Off".into(), |i| format!("Take {}", i + 1)),
    };
    egui::ComboBox::from_id_salt(("take", track))
        .width(70.0)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            for i in (0..n).rev() {
                // ai: edit_comp.action
                ui.selectable_value(&mut pick, Some(i), format!("Take {}", i + 1));
            }
            ui.selectable_value(&mut pick, None, "Off");
        })
        .response
        .accessible_name(format!("Take on {}", t.name));
    (pick != now).then_some(pick)
}

fn channel_picker(ui: &mut egui::Ui, id: &str, name: &str, ch: &mut usize, outs: usize) {
    egui::ComboBox::from_id_salt(id)
        .width(60.0)
        .selected_text(format!("{}", *ch + 1))
        .show_ui(ui, |ui| {
            for c in 0..outs {
                // ai: set_audio_device.main_outputs
                ui.selectable_value(ch, c, format!("{}", c + 1));
            }
        })
        .response
        .accessible_name(name);
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.themes.apply(&ctx);
        self.ctl.collect_garbage();
        self.watch_device(&ctx);

        // Meters fall back at about 20 dB a second.
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        let fall = 10f32.powf(-dt);
        for (i, t) in self.tracks.iter_mut().enumerate() {
            let (mut l, mut r) = self.ctl.status().take_track_peak(i);
            let (il, ir) = self.ctl.status().take_input_peak(i);
            // An armed track shows what it would record.
            if t.arm {
                (l, r) = (l.max(il), r.max(ir));
            }
            t.meter = ((t.meter.0 * fall).max(l), (t.meter.1 * fall).max(r));
        }
        let (l, r) = self.ctl.status().take_master_peak();
        self.master_meter = (
            (self.master_meter.0 * fall).max(l),
            (self.master_meter.1 * fall).max(r),
        );

        let dropped: Vec<_> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        for p in dropped {
            if project::is_project(&p) {
                self.open_dropped(&ctx, p);
            } else if daw_collab::bundle::is_bundle(&p) {
                self.import_bundle(p);
            } else if presets::is_preset(&p) {
                self.import_dropped_preset(&p);
            } else if midifile::is_midi(&p) {
                self.import_midi(&p);
            } else {
                self.add_file(&p);
            }
        }
        let mut rebuild = self.finish_imports() | self.finish_takes() | self.finish_notes();
        let (added, why) = self.video.poll(self.rate);
        if added {
            self.workspace.layout.open(workspace::Pane::Video);
        }
        if why.is_some() {
            self.message = why;
        }
        if self.midi.changed() {
            // A device came or went: open them again on a fresh engine.
            self.restart_audio();
        }
        rebuild |= self.processing.finish(&mut self.tracks, &mut self.message);
        for t in &mut self.tracks {
            let layers = t.layers.iter_mut().map(|l| &mut l.slot);
            let inst = (t.instrument.iter_mut()).chain(layers);
            for slot in t.fx.iter_mut().chain(inst.filter_map(|i| i.plugin_mut())) {
                let was_fine = slot.crashed.is_none();
                rebuild |= slot.poll();
                if was_fine && let Some(why) = &slot.crashed {
                    self.message = Some(if slot.instrument {
                        format!(
                            "{}: {why}. It plays nothing; open the track's instrument to \
                             start it again.",
                            slot.name
                        )
                    } else {
                        format!(
                            "{}: {why}. Its audio passes through; open the track's effects \
                             to start it again.",
                            slot.name
                        )
                    });
                }
                // Its buses changed: made again, with its settings.
                if slot.needs_reload {
                    slot.needs_reload = false;
                    match self.effects.reload(slot, self.rate) {
                        Ok(()) => rebuild = true,
                        Err(e) => self.message = Some(format!("{}: {e}", slot.name)),
                    }
                }
            }
        }
        let slots = (self.tracks.iter_mut()).flat_map(|t| {
            let layers = t.layers.iter_mut().map(|l| &mut l.slot);
            let inst = (t.instrument.iter_mut()).chain(layers);
            t.fx.iter_mut().chain(inst.filter_map(|i| i.plugin_mut()))
        });
        if let Some(m) = self.effects.cap_editors(slots) {
            self.message = Some(m);
        }
        if let Some(e) = self.effects.poll_scan() {
            self.message = Some(e);
        }
        if rebuild {
            self.rebuild();
        }
        if self.effects.scanning().is_some() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        if self.tracks.iter().any(|t| t.arm || t.monitor) {
            // Input meters move while stopped.
            ctx.request_repaint_after(Duration::from_millis(30));
        }
        if !self.imports.is_empty() || self.processing.busy() > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.command_keys(&ctx);
        // Controls with a lane show the lane's value while it is drawn.
        self.show_automation();

        egui::Panel::top("top bar").show(ui, |ui| self.top_bar(ui));
        if let Some(m) = self.message.clone() {
            egui::Panel::bottom("message").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(ui.visuals().error_fg_color, m);
                    // ai: view
                    if ui.button("OK").clicked() {
                        self.message = None;
                    }
                });
            });
        }
        if self.crashes.new > 0 {
            egui::Panel::bottom("crash note").show(ui, |ui| self.crashes.note(ui));
        }
        if self.updates.newer().is_some() {
            egui::Panel::bottom("update note").show(ui, |ui| self.updates.note(ui));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).inner_margin(2.0))
            .show(ui, |ui| self.panes(ui));
        self.latency_window(&ctx);
        self.settings_window(&ctx);
        self.audio_setup_update(&ctx);
        self.effects.credits_window(&ctx);
        self.said.show(&ctx);
        self.poll_freezing(&ctx);
        self.poll_renders(&ctx);
        self.export.poll();
        self.run_export_queue();
        if self.export.busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let exports = match &self.proj.folder {
            Some(f) => f.dir.join("Exports"),
            None => user_dir().join(APP_NAME).join("Exports"),
        };
        self.export.set_default_dir(exports);
        let loop_text = format!("bars {} to {}", self.loop_bars.0, self.loop_bars.1);
        let buses = self.tracks.iter().any(|t| t.kind.is_bus());
        let at = self.ctl.status().position().0;
        let film = self.video.current(at).map(|c| c.name.clone());
        let sections = self.sections();
        let r = match self
            .export
            .window(&ctx, &loop_text, buses, film.as_deref(), &sections)
        {
            Some(export::Action::Export(req)) => self.export_or_queue(req),
            Some(export::Action::Queue(mut req)) => {
                if req.loop_only {
                    req.bars = Some(self.loop_bars);
                }
                self.export.enqueue(req);
                Ok(())
            }
            None => Ok(()),
        };
        if let Err(e) = r {
            self.message = Some(e);
        }
        self.browser_update(&ctx);
        self.tuner.update(&mut self.ctl, self.rate);
        if self.tuner.open {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
        let names: Vec<_> = (self.tracks.iter())
            .map(|t| (t.key.as_str(), t.name.as_str()))
            .collect();
        if let Some(c) = self.meters.window(&ctx, &names) {
            self.send(c);
        }
        let playing = self.ctl.status().playing();
        self.meters.update(&mut self.ctl, self.rate, playing);
        if self.meters.open {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
        self.perf_window(&ctx);
        self.kits_update();
        self.layout_windows(&ctx);
        self.command_windows(&ctx);
        self.updates.poll(&ctx);
        self.updates.window(&ctx);
        self.updates.ask_window(&ctx);
        self.crashes.window(&ctx);
        self.start_window(&ctx);
        self.project_windows(&ctx);
        self.interchange_windows(&ctx);
        self.people_windows(&ctx);
        self.snapshots_window(&ctx);
        self.history_window(&ctx);
        self.review_window(&ctx);
        self.versions_window(&ctx);
        self.tempo_window(&ctx);
        self.hit_points_window(&ctx);
        self.select_notes_window(&ctx);
        self.midi_learn_window(&ctx);
        self.groups_window(&ctx);
        self.time_window(&ctx);
        self.sections_window(&ctx);
        self.arrangement_tick();
        self.timing_window(&ctx);
        self.stretch_windows(&ctx);
        self.transpose_window(&ctx);
        self.pitch_tick(&ctx);
        self.lyrics_tick(&ctx);
        self.remote_tick(&ctx);
        self.assistants_tick(&ctx);
        self.listen_tick(&ctx);
        self.template_windows(&ctx);
        self.preset_windows(&ctx);
        self.parts_windows(&ctx);
        self.poll_loading(&ctx);
        self.poll_sharing(&ctx);
        self.log_practice(&ctx);
        self.automation_tick(&ctx);
        // After the lanes' values are taken off the controls, so an
        // assistant sees and edits the controls' own values.
        self.agent_tick(&ctx);
        self.sync_doc(&ctx, false);
        self.autosave(&ctx);
        self.title(&ctx);
        self.on_close(&ctx);
        if self.ctl.status().playing() {
            ctx.request_repaint();
        }
    }
}

/// `--check-plugins`: scans the standard folders, checks each plugin
/// found and writes the report to `out`. Returns the exit code.
fn check_plugins(out: &std::path::Path) -> i32 {
    let Ok(exe) = std::env::current_exe() else {
        return 1;
    };
    let mut list = daw_plugins::scan::PluginList::default();
    list.rescan(
        &daw_plugins::scan::default_dirs(),
        |b| daw_plugins::scan::scan_in_child(&exe, b, daw_plugins::scan::SCAN_TIMEOUT),
        |b| eprintln!("scanning {}", b.display()),
    );
    let reports: Vec<_> = (list.plugins().iter())
        .map(|p| {
            eprintln!("checking {}", p.class.name);
            daw_plugins::check::check(&exe, &p.bundle, &p.class)
        })
        .collect();
    let mut text = fx::check_report(&reports);
    for (bundle, why) in list.failures() {
        text.push_str(&format!(
            "\nCould not be loaded: {} ({why})",
            bundle.display()
        ));
    }
    match std::fs::write(out, text) {
        Ok(()) => {
            eprintln!("report written to {}", out.display());
            0
        }
        Err(e) => {
            eprintln!("could not write {}: {e}", out.display());
            1
        }
    }
}

fn main() -> eframe::Result {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    if let [flag, bundle] = args.as_slice()
        && flag == daw_plugins::scan::SCAN_ARG
    {
        std::process::exit(daw_plugins::scan::child_main(bundle.as_ref()));
    }
    // Plugins run in a copy of the app (roadmap 2.4), so a crash in one
    // ends that copy, not this one.
    if let [flag] = args.as_slice()
        && flag == daw_plugins::host::HOST_ARG
    {
        std::process::exit(daw_plugins::host::helper_main());
    }
    // Checks every installed plugin for the compatibility list and
    // writes the report (to the file named, or plugin-check.md here).
    if let [flag, rest @ ..] = args.as_slice()
        && flag == "--check-plugins"
        && rest.len() <= 1
    {
        let out = rest
            .first()
            .map_or("plugin-check.md".into(), std::path::PathBuf::from);
        std::process::exit(check_plugins(&out));
    }
    // The app with no window, for assistants and tests.
    if args.first().is_some_and(|a| a == headless::ARG) {
        let rest: Vec<String> = (args[1..].iter())
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        std::process::exit(headless::run(&rest));
    }
    if let [flag] = args.as_slice()
        && flag == "--version"
    {
        println!("{APP_NAME} {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    crash::install(crash::reports_dir());
    let files: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_inner_size([1100.0, 600.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "daw",
        options,
        Box::new(move |_cc| Ok(Box::new(App::new(files)))),
    )
}

#[cfg(test)]
mod mix_ahead_tests {
    use crate::agent::driver::Driver;

    /// A device that isn't there, so the app falls back to playing
    /// silently, split or not as the profile says.
    fn absent() -> DeviceProfile {
        DeviceProfile {
            host: "Test".into(),
            device: "Not plugged in".into(),
            rate: 48_000,
            buffer: Some(64),
            main: (0, 1),
            click: None,
            input: None,
            record_offset: None,
            measured_at: None,
            direct_monitoring: false,
            monitor: daw_audio_io::MonitorOutputs::default(),
            mix_buffer: None,
        }
    }

    use super::DeviceProfile;

    #[test]
    fn the_big_mix_buffer_turns_on_and_off_from_the_command_table() {
        let mut d = Driver::new();
        d.app().profile = Some(absent());
        d.command("add-synth-track");
        {
            // A live track: armed, so it plays in the device callback.
            let app = d.app();
            app.tracks[0].arm = true;
            app.rebuild();
        }
        assert!(!d.app().ctl.wants_twin());

        d.command("mix-buffer");
        let app = d.app();
        assert_eq!(app.profile.as_ref().unwrap().mix_buffer, Some(2048));
        assert!(app.ctl.wants_twin(), "split");
        assert_eq!(app.ctl.status().ahead.block(), Some(2048));
        // The mix thread keeps up, and the device settings say so.
        let start = std::time::Instant::now();
        while app.ctl.status().ahead.queued() == 0 && start.elapsed().as_secs() < 5 {
            std::thread::yield_now();
        }
        assert!(app.ctl.status().ahead.queued() > 0, "the mix runs ahead");
        let line = super::mix_ahead_line(app.ctl.status()).unwrap();
        assert!(line.contains("blocks of 2048 samples"), "{line}");

        d.command("mix-buffer");
        let app = d.app();
        assert_eq!(app.profile.as_ref().unwrap().mix_buffer, None);
        assert!(!app.ctl.wants_twin());
        assert!(super::mix_ahead_line(app.ctl.status()).is_none());
        // Back on at the size last used.
        d.command("mix-buffer");
        assert_eq!(d.app().profile.as_ref().unwrap().mix_buffer, Some(2048));
    }
}
