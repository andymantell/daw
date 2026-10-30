// SPDX-License-Identifier: GPL-3.0-or-later
//! Named commands (roadmap 2.1): every action in the menus and on the
//! keyboard is one, found by typing in the command search (Ctrl+K), with
//! keys the user can change in Settings → Key commands. Keys are kept in
//! `keys.txt` beside the device settings, only where they differ from
//! the defaults.

use std::collections::BTreeMap;
use std::path::PathBuf;

use daw_engine::engine::Command;
use daw_engine::time::Frames;
use eframe::egui::{self, Event, Key, Modifiers, RichText};

use crate::App;
use crate::project::{After, Dialog, find_projects};
use crate::workspace::Pane;

/// Every command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Cmd {
    NewProject,
    NewFromTemplate,
    OpenProject,
    StartScreen,
    Save,
    SaveAs,
    SaveAsTemplate,
    ExportBundle,
    ImportBundle,
    Snapshots,
    People,
    Export,
    Undo,
    Redo,
    Split,
    Delete,
    Marker,
    HitPoint,
    HitPoints,
    Timecode,
    ClearSelection,
    Crossfade,
    ClipFades,
    Copy,
    Paste,
    Bounce,
    EditGroups,
    FixTiming,
    ZoomIn,
    ZoomOut,
    ZoomSelection,
    Snap,
    Ripple,
    VolumeLines,
    GroupEdit,
    PlayStop,
    ToStart,
    Record,
    Loop,
    Metronome,
    CountIn,
    Mixer,
    MonitorSection,
    Talkback,
    Dim,
    Mono,
    TakeLanes,
    AutomationLanes,
    Browser,
    FullSize,
    /// Recalls saved layout n, counted from 0.
    Layout(u8),
    SaveLayout,
    PaneWindows,
    Tuner,
    Meters,
    LessonNotes,
    Lessons,
    Comments,
    NotesEditor,
    PatternEditor,
    AddPattern,
    MakePatternUnique,
    PatternToNotes,
    AddInstrumentTrack,
    /// Adds a track with one of the built-in instruments, by its place
    /// in `daw_instruments::Kind::ALL`.
    AddInstrument(u8),
    InstrumentWindow,
    SliceNotes,
    PadBuses,
    KeepNotes,
    QuantiseNotes,
    HumaniseNotes,
    LegatoNotes,
    FitNotesToScale,
    ScaleMode,
    SelectNotes,
    GrooveFromNotes,
    GrooveFromAudio,
    ApplyGroove,
    MidiLearn,
    Instrument,
    AddLayer,
    SingleInstrument,
    ImportMidi,
    ExportMidi,
    AudioDevice,
    Plugins,
    PerformanceCheck,
    DrumKits,
    LearnKit,
    KitNext,
    KitSkip,
    KitBack,
    KitCancel,
    ImportKit,
    ExportKit,
    KeyCommands,
    Theme,
    NextTheme,
    LightDark,
    FindCommand,
    CheckUpdates,
    CrashReports,
    SoundCredits,
}

/// A key with the modifiers held for it. Ctrl means Cmd on a Mac.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Shortcut {
    pub(crate) mods: Modifiers,
    pub(crate) key: Key,
}

const NONE: Modifiers = Modifiers::NONE;
const CTRL: Modifiers = Modifiers::COMMAND;
const CTRL_SHIFT: Modifiers = Modifiers {
    shift: true,
    ..Modifiers::COMMAND
};

const fn key(mods: Modifiers, key: Key) -> Shortcut {
    Shortcut { mods, key }
}

/// What the user sees of a command, and its keys out of the box.
pub(crate) struct Info {
    pub(crate) cmd: Cmd,
    /// Kept in `keys.txt`: never change one.
    id: &'static str,
    pub(crate) name: &'static str,
    /// The menu it is in.
    pub(crate) group: &'static str,
    /// Said on hover, and searched.
    hint: &'static str,
    /// Other words to find it by.
    words: &'static str,
    keys: &'static [Shortcut],
    /// Its keys work in a text field too.
    typing: bool,
    /// Holding its key down repeats it.
    repeat: bool,
}

const fn info(
    cmd: Cmd,
    id: &'static str,
    group: &'static str,
    name: &'static str,
    keys: &'static [Shortcut],
) -> Info {
    Info {
        cmd,
        id,
        name,
        group,
        hint: "",
        words: "",
        keys,
        typing: false,
        repeat: false,
    }
}

impl Info {
    const fn hint(self, hint: &'static str) -> Info {
        Info { hint, ..self }
    }

    const fn words(self, words: &'static str) -> Info {
        Info { words, ..self }
    }

    const fn typing(self) -> Info {
        Info {
            typing: true,
            ..self
        }
    }

    const fn repeat(self) -> Info {
        Info {
            repeat: true,
            ..self
        }
    }
}

/// Every command, in menu order.
pub(crate) const COMMANDS: &[Info] = &[
    info(Cmd::NewProject, "new-project", "File", "New project", &[]).words("start empty"),
    info(
        Cmd::NewFromTemplate,
        "new-from-template",
        "File",
        "New from template…",
        &[],
    )
    .hint("Start a project with tracks, sections and notes laid out")
    .words("start lesson band demo"),
    info(
        Cmd::OpenProject,
        "open-project",
        "File",
        "Open project…",
        &[],
    )
    .words("load"),
    info(
        Cmd::StartScreen,
        "start-screen",
        "File",
        "Start screen…",
        &[],
    )
    .hint("Recent projects and templates")
    .words("recent welcome begin"),
    info(Cmd::Save, "save", "File", "Save", &[key(CTRL, Key::S)])
        .hint("Save the project")
        .typing(),
    info(Cmd::SaveAs, "save-as", "File", "Save as…", &[]).words("copy rename"),
    info(
        Cmd::SaveAsTemplate,
        "save-as-template",
        "File",
        "Save as template…",
        &[],
    )
    .hint("Keep this project's tracks, effects, sections and notes to start from")
    .words("layout set-up"),
    info(
        Cmd::ExportBundle,
        "export-bundle",
        "File",
        "Export bundle…",
        &[],
    )
    .hint("The project and its audio in one file, to send by any means")
    .words("share send"),
    info(
        Cmd::ImportBundle,
        "import-bundle",
        "File",
        "Import bundle…",
        &[],
    )
    .hint("Merges into your copy of the project if you have one")
    .words("share receive"),
    info(Cmd::Snapshots, "snapshots", "File", "Snapshots…", &[])
        .hint("Name the project as it is now, or go back to a named moment")
        .words("restore version history"),
    info(Cmd::People, "people", "File", "People…", &[])
        .hint("Who has this project, and what they may change")
        .words("roles join name"),
    info(Cmd::Export, "export", "File", "Export…", &[])
        .hint("The mix, or each track or bus as a stem, as WAV, FLAC, MP3 or Opus")
        .words("render bounce stems wav flac mp3 opus ogg mixdown bus"),
    info(
        Cmd::ImportMidi,
        "import-midi",
        "File",
        "Import MIDI file…",
        &[],
    )
    .hint("Each track in the file becomes an instrument track with its notes")
    .words("notes smf mid"),
    info(
        Cmd::ExportMidi,
        "export-midi",
        "File",
        "Export MIDI file…",
        &[],
    )
    .hint("The instrument tracks' notes as a standard MIDI file")
    .words("notes smf mid"),
    info(
        Cmd::AddInstrumentTrack,
        "add-instrument-track",
        "Edit",
        "Add instrument track",
        &[],
    )
    .hint("A track that plays notes through an instrument, from a MIDI keyboard or the note editor")
    .words("midi synth notes"),
    info(Cmd::AddInstrument(0), "add-synth-track", "Edit", "Add synth track", &[])
        .hint("An instrument track with the synth: two oscillators, a filter with its own envelope, an LFO")
        .words("instrument poly unison pad lead bass"),
    info(Cmd::AddInstrument(1), "add-sampler-track", "Edit", "Add sampler track", &[])
        .hint("An instrument track that plays your sound files over the keys")
        .words("instrument multisample zones keys"),
    info(
        Cmd::AddInstrument(2),
        "add-drum-sampler-track",
        "Edit",
        "Add drum sampler track",
        &[],
    )
    .hint("An instrument track that plays one sound file as a hit")
    .words("instrument one shot sample"),
    info(
        Cmd::AddInstrument(3),
        "add-drum-machine-track",
        "Edit",
        "Add drum machine track",
        &[],
    )
    .hint("Sixteen pads of drums, layered, with choke groups, effects and outputs per pad")
    .words("instrument drums kit beat pads"),
    info(
        Cmd::AddInstrument(4),
        "add-slicer-track",
        "Edit",
        "Add loop slicer track",
        &[],
    )
    .hint("Cuts a loop at each hit and plays the slices from the keys")
    .words("instrument loop break chop slices"),
    info(
        Cmd::InstrumentWindow,
        "instrument-window",
        "View",
        "Instrument…",
        &[],
    )
    .hint("The instrument of the instrument track, and its controls")
    .words("synth sampler drum machine slicer settings"),
    info(
        Cmd::SliceNotes,
        "slice-notes",
        "Edit",
        "Make notes that play the loop",
        &[],
    )
    .hint("On the loop slicer's track: a note clip that plays its slices in order, at the song's tempo")
    .words("slicer loop tempo chop"),
    info(
        Cmd::PadBuses,
        "pad-buses",
        "Edit",
        "Put each drum pad on a bus",
        &[],
    )
    .hint("On the drum machine's track: each pad with a sound gets an output and a bus in the mixer")
    .words("drum machine outputs multi out mixer"),
    info(
        Cmd::NotesEditor,
        "notes-editor",
        "View",
        "Note editor",
        &[key(NONE, Key::E)],
    )
    .hint("Draw, move and shape the notes of the selected note clip")
    .words("piano roll midi notes velocity quantise"),
    info(
        Cmd::QuantiseNotes,
        "quantise-notes",
        "Notes",
        "Quantise notes",
        &[key(NONE, Key::Q)],
    )
    .hint("Move the note editor's selected notes (or all) towards the grid, by its strength and swing")
    .words("quantize grid timing"),
    info(
        Cmd::HumaniseNotes,
        "humanise-notes",
        "Notes",
        "Humanise notes",
        &[],
    )
    .hint("Move the note editor's selected notes (or all) a little, at random, in time, velocity and length")
    .words("humanize random feel loosen"),
    info(Cmd::LegatoNotes, "legato-notes", "Notes", "Legato", &[])
        .hint("Make each of the note editor's selected notes (or all) last until the next starts")
        .words("join length tie"),
    info(
        Cmd::FitNotesToScale,
        "fit-notes-to-scale",
        "Notes",
        "Fit notes to the scale",
        &[],
    )
    .hint("Move notes off the key's scale to the nearest note on it")
    .words("key correct wrong notes"),
    info(Cmd::ScaleMode, "scale-mode", "Notes", "Scale mode", &[])
        .hint("Keep drawn and moved notes on the key's scale")
        .words("key snap pitch"),
    info(Cmd::SelectNotes, "select-notes", "Notes", "Select notes…", &[])
        .hint("Choose notes by time, pitch, velocity, chance or length, or the ones off the scale")
        .words("filter find choose range"),
    info(
        Cmd::GrooveFromNotes,
        "groove-from-notes",
        "Notes",
        "Make a groove from these notes",
        &[],
    )
    .hint("Take the timing and accents of the note editor's selected notes (or all) as a groove")
    .words("template feel swing extract"),
    info(
        Cmd::GrooveFromAudio,
        "groove-from-audio",
        "Notes",
        "Make a groove from the selected audio",
        &[],
    )
    .hint("Take the timing and accents of the selected audio clips as a groove")
    .words("template feel swing extract drums"),
    info(Cmd::ApplyGroove, "apply-groove", "Notes", "Apply groove", &[])
        .hint("Move the note editor's selected notes (or all) towards the groove chosen")
        .words("template feel swing quantise"),
    info(Cmd::MidiLearn, "midi-learn", "Notes", "MIDI learn…", &[])
        .hint("Tie a knob, fader or pedal on a MIDI device to a setting")
        .words("controller map knob fader assign"),
    info(Cmd::Instrument, "instrument", "Notes", "Instrument…", &[])
        .hint("The instrument track's instrument, or its layers, and their settings")
        .words("synth sound layers"),
    info(Cmd::AddLayer, "add-layer", "Notes", "Add an instrument layer", &[])
        .hint("Another instrument playing the instrument track's notes: for a keyboard split or a doubled sound")
        .words("layered instruments stack split zone double rack"),
    info(
        Cmd::SingleInstrument,
        "single-instrument",
        "Notes",
        "Back to one instrument",
        &[],
    )
    .hint("Keep only the first layer's instrument on the instrument track")
    .words("layers unlayer remove"),
    info(
        Cmd::PatternEditor,
        "pattern-editor",
        "View",
        "Pattern editor",
        &[key(NONE, Key::P)],
    )
    .hint("Program the steps of a drum pattern: swing, chance, repeats and fills")
    .words("step sequencer drums beat euclidean ratchet probability swing"),
    info(Cmd::AddPattern, "add-pattern", "Edit", "Add drum pattern", &[])
        .hint("A clip playing a new drum pattern at the playhead, on the selected clip's instrument track")
        .words("step sequencer beat drums pattern clip"),
    info(
        Cmd::MakePatternUnique,
        "make-pattern-unique",
        "Edit",
        "Make pattern unique",
        &[],
    )
    .hint("Give the selected pattern clip a copy of its pattern of its own")
    .words("unlink linked pattern copy"),
    info(
        Cmd::PatternToNotes,
        "pattern-to-notes",
        "Edit",
        "Convert pattern to plain notes",
        &[],
    )
    .hint("Keep the notes the selected pattern clip plays as notes, no longer following the pattern")
    .words("flatten unlink pattern notes"),
    info(
        Cmd::KeepNotes,
        "keep-notes",
        "Transport",
        "Keep the notes I just played",
        &[],
    )
    .hint("Makes a note clip from what was played on a MIDI keyboard without Record on")
    .words("retrospective capture midi"),
    info(Cmd::Undo, "undo", "Edit", "Undo", &[key(CTRL, Key::Z)]).repeat(),
    info(
        Cmd::Redo,
        "redo",
        "Edit",
        "Redo",
        &[key(CTRL_SHIFT, Key::Z), key(CTRL, Key::Y)],
    )
    .repeat(),
    info(
        Cmd::Split,
        "split",
        "Edit",
        "Split at playhead",
        &[key(NONE, Key::S)],
    )
    .hint("Split clips at the playhead: the selected ones, or all under it")
    .words("cut"),
    info(
        Cmd::Delete,
        "delete",
        "Edit",
        "Delete selected clips",
        &[key(NONE, Key::Delete), key(NONE, Key::Backspace)],
    )
    .hint("Delete the selected clips, or the focused marker")
    .words("remove erase"),
    info(
        Cmd::Marker,
        "marker",
        "Edit",
        "Mark a section",
        &[key(NONE, Key::M)],
    )
    .hint("Mark a section at the playhead's bar")
    .words("marker add"),
    info(
        Cmd::HitPoint,
        "hit-point",
        "Edit",
        "Add a hit point",
        &[key(NONE, Key::H)],
    )
    .hint("Mark a moment in the picture at the playhead, on the start of its frame")
    .words("hit point cue sync video film picture"),
    info(
        Cmd::HitPoints,
        "hit-points",
        "Edit",
        "Timecode and hit points…",
        &[],
    )
    .hint("Timecode, hit points, and a tempo that lands hit points on beats or bars")
    .words("tempo fit smpte frame rate video film picture score"),
    info(Cmd::Timecode, "timecode", "View", "Timecode", &[])
        .hint("Show timecode on the ruler and in the transport bar")
        .words("smpte frames hours minutes seconds"),
    info(
        Cmd::ClearSelection,
        "clear-selection",
        "Edit",
        "Clear the selection",
        &[key(NONE, Key::Escape)],
    )
    .words("deselect none"),
    info(
        Cmd::Crossfade,
        "crossfade",
        "Edit",
        "Crossfade where clips meet",
        &[key(NONE, Key::X)],
    )
    .hint("Fade clips into each other where they meet: the selected ones, or all")
    .words("join blend overlap"),
    info(
        Cmd::ClipFades,
        "clip-fades",
        "Edit",
        "Fades and volume…",
        &[key(NONE, Key::F)],
    )
    .hint("Fade in, fade out and volume of the selected clips")
    .words("gain level fade curve envelope"),
    info(
        Cmd::Copy,
        "copy",
        "Edit",
        "Copy clips",
        &[key(CTRL, Key::C)],
    )
    .hint("Copy the selected clips, to paste at the playhead or onto a track")
    .words("duplicate clipboard"),
    info(
        Cmd::Paste,
        "paste",
        "Edit",
        "Paste clips",
        &[key(CTRL, Key::V)],
    )
    .hint("Paste the copied clips at the playhead, each on the track it came from")
    .words("duplicate clipboard"),
    info(Cmd::Bounce, "bounce", "Edit", "Bounce to a new track", &[])
        .hint(
            "The selected clips through their track's effects, as new audio on a track beside \
             it; they are muted where they are",
        )
        .words("render in place consolidate print"),
    info(Cmd::EditGroups, "edit-groups", "Edit", "Edit groups…", &[])
        .hint("Group tracks, such as drum mics, so their clips are edited together")
        .words("link tracks drums multitrack"),
    info(Cmd::FixTiming, "fix-timing", "Edit", "Fix timing…", &[])
        .hint("Cut the selected clips at each note and move the notes toward the grid")
        .words("quantise quantize tighten align beat slice transient attack groove"),
    info(
        Cmd::ZoomIn,
        "zoom-in",
        "View",
        "Zoom in",
        &[key(NONE, Key::Equals)],
    )
    .repeat()
    .words("magnify closer"),
    info(
        Cmd::ZoomOut,
        "zoom-out",
        "View",
        "Zoom out",
        &[key(NONE, Key::Minus)],
    )
    .repeat()
    .words("further"),
    info(
        Cmd::ZoomSelection,
        "zoom-selection",
        "View",
        "Zoom to the selected clips",
        &[key(NONE, Key::Z)],
    )
    .hint("Show the selected clips across the timeline, or the whole song")
    .words("fit"),
    info(
        Cmd::Snap,
        "snap",
        "View",
        "Snap on or off",
        &[key(NONE, Key::N)],
    )
    .hint("Snap clips to the grid while dragging")
    .words("grid magnet"),
    info(
        Cmd::Ripple,
        "ripple",
        "View",
        "Ripple editing on or off",
        &[],
    )
    .hint("Deleting a clip, or trimming its end, moves the later clips on its track")
    .words("close gaps shift"),
    info(
        Cmd::VolumeLines,
        "volume-lines",
        "View",
        "Volume lines on or off",
        &[],
    )
    .hint("Show each clip's volume line, to drag and add points to")
    .words("gain envelope automation"),
    info(
        Cmd::GroupEdit,
        "group-edit",
        "View",
        "Group editing on or off",
        &[key(NONE, Key::G)],
    )
    .hint("Edit the clips on grouped tracks together, or one track alone")
    .words("link drums suspend"),
    info(
        Cmd::PlayStop,
        "play",
        "Transport",
        "Play or stop",
        &[key(NONE, Key::Space)],
    )
    .words("start pause"),
    info(
        Cmd::ToStart,
        "to-start",
        "Transport",
        "Back to the start",
        &[key(NONE, Key::Home)],
    )
    .words("rewind beginning"),
    info(
        Cmd::Record,
        "record",
        "Transport",
        "Record on or off",
        &[key(NONE, Key::R)],
    )
    .hint("Armed tracks record while playing"),
    info(
        Cmd::Loop,
        "loop",
        "Transport",
        "Loop on or off",
        &[key(NONE, Key::L)],
    )
    .words("repeat cycle"),
    info(
        Cmd::Metronome,
        "metronome",
        "Transport",
        "Metronome on or off",
        &[key(NONE, Key::K)],
    )
    .words("click"),
    info(
        Cmd::CountIn,
        "count-in",
        "Transport",
        "Count-in on or off",
        &[key(NONE, Key::C)],
    )
    .hint("One bar of clicks before playing")
    .words("pre-roll"),
    info(Cmd::Mixer, "mixer", "View", "Mixer", &[])
        .hint("Show or hide the mixer")
        .words("pane faders tracks"),
    info(
        Cmd::MonitorSection,
        "monitor-section",
        "View",
        "Monitor section",
        &[],
    )
    .hint("Show or hide the headphone mixes and the speakers' controls")
    .words("cue headphones talkback dim mono speakers control room"),
    info(
        Cmd::Talkback,
        "talkback",
        "Transport",
        "Talkback on or off",
        &[],
    )
    .hint("Talk to the headphones through the talkback microphone")
    .words("talk microphone headphones cue"),
    info(Cmd::Dim, "dim", "Transport", "Dim the speakers", &[])
        .hint("Turn the speakers down, or back up")
        .words("quiet control room monitor"),
    info(Cmd::Mono, "mono", "Transport", "Mono on or off", &[])
        .hint("Hear the speakers in mono, to check the mix")
        .words("sum mono check control room"),
    info(Cmd::TakeLanes, "take-lanes", "View", "Take lanes", &[])
        .hint("Show or hide each take of a track, to comp from")
        .words("comp comping takes loop recording swipe best"),
    info(
        Cmd::AutomationLanes,
        "automation-lanes",
        "View",
        "Automation",
        &[],
    )
    .hint("Show or hide a track's automation lanes: settings that follow a line as the song plays")
    .words("lanes volume pan fader read touch latch write envelope"),
    info(
        Cmd::Browser,
        "browser",
        "View",
        "Content browser",
        &[key(CTRL, Key::B)],
    )
    .hint("Show or hide your sound folders and the project's audio, to hear and add")
    .words("samples loops sounds library files search tags preview audition"),
    info(
        Cmd::FullSize,
        "full-size",
        "View",
        "Full size",
        &[key(CTRL_SHIFT, Key::F)],
    )
    .hint("Show the pane last clicked in full size, or all panes again")
    .words("maximise maximize zoom pane"),
    info(
        Cmd::Layout(0),
        "layout-1",
        "View",
        "Layout 1",
        &[key(CTRL, Key::Num1)],
    )
    .hint("Recall saved layout 1")
    .words("workspace arrangement panes"),
    info(
        Cmd::Layout(1),
        "layout-2",
        "View",
        "Layout 2",
        &[key(CTRL, Key::Num2)],
    )
    .hint("Recall saved layout 2")
    .words("workspace arrangement panes"),
    info(
        Cmd::Layout(2),
        "layout-3",
        "View",
        "Layout 3",
        &[key(CTRL, Key::Num3)],
    )
    .hint("Recall saved layout 3")
    .words("workspace arrangement panes"),
    info(
        Cmd::Layout(3),
        "layout-4",
        "View",
        "Layout 4",
        &[key(CTRL, Key::Num4)],
    )
    .hint("Recall saved layout 4")
    .words("workspace arrangement panes"),
    info(
        Cmd::Layout(4),
        "layout-5",
        "View",
        "Layout 5",
        &[key(CTRL, Key::Num5)],
    )
    .hint("Recall saved layout 5")
    .words("workspace arrangement panes"),
    info(
        Cmd::Layout(5),
        "layout-6",
        "View",
        "Layout 6",
        &[key(CTRL, Key::Num6)],
    )
    .hint("Recall saved layout 6")
    .words("workspace arrangement panes"),
    info(
        Cmd::Layout(6),
        "layout-7",
        "View",
        "Layout 7",
        &[key(CTRL, Key::Num7)],
    )
    .hint("Recall saved layout 7")
    .words("workspace arrangement panes"),
    info(
        Cmd::Layout(7),
        "layout-8",
        "View",
        "Layout 8",
        &[key(CTRL, Key::Num8)],
    )
    .hint("Recall saved layout 8")
    .words("workspace arrangement panes"),
    info(
        Cmd::Layout(8),
        "layout-9",
        "View",
        "Layout 9",
        &[key(CTRL, Key::Num9)],
    )
    .hint("Recall saved layout 9")
    .words("workspace arrangement panes"),
    info(Cmd::SaveLayout, "save-layout", "View", "Save layout…", &[])
        .hint("Name the arrangement of panes now, to recall with a key")
        .words("workspace panes"),
    info(
        Cmd::PaneWindows,
        "pane-windows",
        "View",
        "Panes in their own windows",
        &[],
    )
    .hint("Let a pane open in a window of its own, to put on another screen")
    .words("detach screens monitors undock float tear"),
    info(Cmd::Tuner, "tuner", "Practice", "Tuner…", &[]).words("tune pitch"),
    info(Cmd::Meters, "meters", "Practice", "Meters…", &[])
        .words("loudness lufs level peak rms spectrum phase"),
    info(
        Cmd::LessonNotes,
        "lesson-notes",
        "Practice",
        "Lesson notes…",
        &[],
    )
    .words("tab text"),
    info(Cmd::Lessons, "lessons", "Practice", "Lessons", &[])
        .hint("Show or hide assignments, the practice log and the lessons so far")
        .words("assignment homework practice log history progress teacher student"),
    info(Cmd::Comments, "comments", "Practice", "Comments…", &[]).words("pins feedback"),
    info(
        Cmd::AudioDevice,
        "audio-device",
        "Settings",
        "Audio device…",
        &[],
    )
    .words("interface sound card input output latency"),
    info(Cmd::Plugins, "plugins", "Settings", "Plugins…", &[]).words("effects scan"),
    info(
        Cmd::PerformanceCheck,
        "performance-check",
        "Settings",
        "Performance check…",
        &[],
    )
    .words("dsp cpu load"),
    info(Cmd::DrumKits, "drum-kits", "Settings", "Drum kits…", &[])
        .hint("Which note each pad of an electronic drum kit sends")
        .words("electronic e-kit pads map midi notes profile"),
    info(
        Cmd::LearnKit,
        "learn-kit",
        "Settings",
        "Learn a drum kit…",
        &[],
    )
    .hint("Hit each pad of an electronic kit when asked, to learn its notes")
    .words("electronic e-kit pads map midi notes profile wizard"),
    info(
        Cmd::KitNext,
        "kit-next",
        "Settings",
        "Drum kit: next step",
        &[],
    )
    .hint("Keep what was heard and go on (saves at the last step)")
    .words("learn kit"),
    info(
        Cmd::KitSkip,
        "kit-skip",
        "Settings",
        "Drum kit: skip this pad",
        &[],
    )
    .hint("Go on without this pad or zone: it isn't on the kit")
    .words("learn kit"),
    info(
        Cmd::KitBack,
        "kit-back",
        "Settings",
        "Drum kit: back a step",
        &[],
    )
    .hint("Learn the step before again")
    .words("learn kit"),
    info(
        Cmd::KitCancel,
        "kit-cancel",
        "Settings",
        "Drum kit: stop learning",
        &[],
    )
    .words("learn kit cancel"),
    info(
        Cmd::ImportKit,
        "import-kit",
        "Settings",
        "Import drum kit…",
        &[],
    )
    .hint("Add a drum kit profile from a file")
    .words("electronic e-kit pads map profile"),
    info(
        Cmd::ExportKit,
        "export-kit",
        "Settings",
        "Export drum kit…",
        &[],
    )
    .hint("Save the chosen drum kit profile as a file, to share it")
    .words("electronic e-kit pads map profile"),
    info(
        Cmd::KeyCommands,
        "key-commands",
        "Settings",
        "Key commands…",
        &[],
    )
    .hint("See and change the key for each command")
    .words("keyboard shortcuts keys"),
    info(Cmd::Theme, "theme", "Settings", "Theme…", &[])
        .hint("Choose how the app looks on this computer")
        .words("look colours colors appearance skin dark light"),
    info(Cmd::NextTheme, "next-theme", "Settings", "Next theme", &[])
        .words("look colours colors appearance skin switch"),
    info(
        Cmd::LightDark,
        "light-dark",
        "Settings",
        "Switch light and dark",
        &[],
    )
    .hint("Swap the theme's light and dark colours")
    .words("night day mode appearance"),
    info(
        Cmd::FindCommand,
        "find-command",
        "Help",
        "Find a command…",
        &[key(CTRL, Key::K)],
    )
    .hint("Type part of a command's name and press Enter to run it")
    .words("search palette")
    .typing(),
    info(
        Cmd::CheckUpdates,
        "check-updates",
        "Help",
        "Check for updates…",
        &[],
    )
    .words("version new"),
    info(
        Cmd::CrashReports,
        "crash-reports",
        "Help",
        "Crash reports…",
        &[],
    ),
    info(
        Cmd::SoundCredits,
        "sound-credits",
        "Help",
        "Sound credits…",
        &[],
    )
    .words("reverb rooms licence attribution"),
];

impl Cmd {
    pub(crate) fn info(self) -> &'static Info {
        COMMANDS
            .iter()
            .find(|i| i.cmd == self)
            .expect("every command is in the table")
    }

    fn from_id(id: &str) -> Option<Cmd> {
        COMMANDS.iter().find(|i| i.id == id).map(|i| i.cmd)
    }

    /// Changes the workspace.
    pub(crate) fn on_workspace(self) -> bool {
        matches!(
            self,
            Cmd::Mixer
                | Cmd::MonitorSection
                | Cmd::TakeLanes
                | Cmd::Lessons
                | Cmd::NotesEditor
                | Cmd::PatternEditor
                | Cmd::AutomationLanes
                | Cmd::Browser
                | Cmd::FullSize
                | Cmd::Layout(_)
                | Cmd::SaveLayout
                | Cmd::PaneWindows
        )
    }

    /// Run by the timeline, which knows what has the keyboard there.
    pub(crate) fn on_timeline(self) -> bool {
        matches!(
            self,
            Cmd::Split
                | Cmd::Delete
                | Cmd::Marker
                | Cmd::ClearSelection
                | Cmd::Crossfade
                | Cmd::ClipFades
                | Cmd::ZoomIn
                | Cmd::ZoomOut
                | Cmd::ZoomSelection
                | Cmd::Snap
                | Cmd::Ripple
                | Cmd::VolumeLines
                | Cmd::GroupEdit
        )
    }
}

/// Keys that work the same everywhere and can't be given to a command.
/// The timeline's own keys act on the focused clip or marker.
pub(crate) const FIXED: &[(&str, &str)] = &[
    (
        "Tab, Shift+Tab",
        "Move between controls, and through the timeline's clips and markers",
    ),
    (
        "Enter",
        "Press the focused button or open the focused list. On a clip: select it and go to its start. On a marker: go to its section",
    ),
    (
        "Ctrl+Enter",
        "On a clip: add it to or take it out of the selection. On a marker: loop its section",
    ),
    (
        "Left, Right",
        "Move the selected clips a grid step earlier or later; a marker a bar",
    ),
    ("Shift+Left, Shift+Right", "Move the selected clips a bar"),
    (
        "Ctrl+Left, Ctrl+Right",
        "Trim the focused clip's start a grid step (with Shift, a bar)",
    ),
    (
        "Alt+Left, Alt+Right",
        "Trim the focused clip's end a grid step (with Shift, a bar)",
    ),
    (
        "Up, Down",
        "Move to the nearest clip on the track above or below",
    ),
    (
        "Ctrl+Up, Ctrl+Down",
        "Move the selected clips to the track above or below",
    ),
    ("F2", "Rename the focused marker"),
];

impl Shortcut {
    /// As pressed: Ctrl and Cmd count as one.
    fn pressed(key: Key, m: Modifiers) -> Shortcut {
        Shortcut {
            key,
            mods: Modifiers {
                alt: m.alt,
                shift: m.shift,
                command: m.command || m.ctrl || m.mac_cmd,
                ctrl: false,
                mac_cmd: false,
            },
        }
    }

    /// Why this key can't be given to a command, if it can't.
    fn fixed(self) -> Option<&'static str> {
        match self.key {
            Key::Tab => Some("Tab moves between controls"),
            Key::Enter => Some("Enter presses the focused control"),
            Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown => {
                Some("The arrow keys move and trim what has the keyboard")
            }
            Key::F2 if self.mods == NONE => Some("F2 renames the focused marker"),
            _ => None,
        }
    }

    /// Such as `Ctrl+Shift+Z`, as shown and as kept in `keys.txt`.
    pub(crate) fn text(self) -> String {
        let mut t = String::new();
        if self.mods.command {
            t.push_str(if cfg!(target_os = "macos") {
                "Cmd+"
            } else {
                "Ctrl+"
            });
        }
        if self.mods.alt {
            t.push_str("Alt+");
        }
        if self.mods.shift {
            t.push_str("Shift+");
        }
        t.push_str(self.key.name());
        t
    }

    pub(crate) fn parse(text: &str) -> Option<Shortcut> {
        let mut parts: Vec<&str> = text.trim().split('+').map(str::trim).collect();
        let key = Key::from_name(parts.pop()?)?;
        let mut mods = NONE;
        for p in parts {
            match p.to_ascii_lowercase().as_str() {
                "ctrl" | "cmd" => mods.command = true,
                "alt" => mods.alt = true,
                "shift" => mods.shift = true,
                _ => return None,
            }
        }
        Some(Shortcut { mods, key })
    }
}

/// The keys of every command.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Keymap {
    keys: BTreeMap<Cmd, Vec<Shortcut>>,
}

impl Default for Keymap {
    fn default() -> Keymap {
        Keymap {
            keys: COMMANDS.iter().map(|i| (i.cmd, i.keys.to_vec())).collect(),
        }
    }
}

impl Keymap {
    /// One line per command whose keys differ from the defaults: its id,
    /// then its keys separated by commas, or `none`.
    pub(crate) fn from_text(text: &str) -> Keymap {
        let mut map = Keymap::default();
        for line in text.lines() {
            let (id, rest) = line.trim().split_once(' ').unwrap_or((line.trim(), ""));
            let Some(cmd) = Cmd::from_id(id) else {
                continue;
            };
            let keys = match rest.trim() {
                "none" => Vec::new(),
                rest => rest
                    .split(',')
                    .filter_map(Shortcut::parse)
                    .filter(|s| s.fixed().is_none())
                    .collect(),
            };
            map.keys.insert(cmd, keys);
        }
        map
    }

    pub(crate) fn to_text(&self) -> String {
        let mut t = String::new();
        for i in COMMANDS {
            let keys = self.of(i.cmd);
            if keys != i.keys {
                let list: Vec<String> = keys.iter().map(|s| s.text()).collect();
                let list = if list.is_empty() {
                    "none".to_string()
                } else {
                    list.join(", ")
                };
                t.push_str(&format!("{} {list}\n", i.id));
            }
        }
        t
    }

    pub(crate) fn of(&self, cmd: Cmd) -> &[Shortcut] {
        self.keys.get(&cmd).map_or(&[], Vec::as_slice)
    }

    /// Its keys, as shown beside it.
    pub(crate) fn text(&self, cmd: Cmd) -> String {
        let keys: Vec<String> = self.of(cmd).iter().map(|s| s.text()).collect();
        keys.join(", ")
    }

    /// The command a key runs, in menu order if two share it.
    pub(crate) fn find(&self, s: Shortcut) -> Option<Cmd> {
        COMMANDS
            .iter()
            .map(|i| i.cmd)
            .find(|c| self.of(*c).contains(&s))
    }

    /// Gives the key to `cmd`, taking it from any other command.
    pub(crate) fn assign(&mut self, cmd: Cmd, s: Shortcut) {
        for keys in self.keys.values_mut() {
            keys.retain(|k| *k != s);
        }
        self.keys.entry(cmd).or_default().push(s);
    }

    pub(crate) fn remove(&mut self, cmd: Cmd, s: Shortcut) {
        if let Some(keys) = self.keys.get_mut(&cmd) {
            keys.retain(|k| *k != s);
        }
    }

    /// Back to its default keys, taking them from any command that has
    /// one of them now.
    pub(crate) fn reset(&mut self, cmd: Cmd) {
        self.keys.insert(cmd, Vec::new());
        for &s in cmd.info().keys {
            self.assign(cmd, s);
        }
    }
}

/// Commands matching what was typed, best first: every word typed must
/// begin a word of the command's name, menu or other words, or appear
/// in its name. Nothing typed lists them all, in menu order.
#[cfg(test)]
pub(crate) fn search(query: &str) -> Vec<Cmd> {
    search_named(query, &BTreeMap::new())
}

/// As `search`, with some commands under the names in `names` (a
/// layout's, say) as well as their own.
pub(crate) fn search_named(query: &str, names: &BTreeMap<Cmd, String>) -> Vec<Cmd> {
    let query = query.trim().to_lowercase();
    let typed: Vec<&str> = query.split_whitespace().collect();
    let mut found: Vec<(u32, usize, Cmd)> = Vec::new();
    for (order, i) in COMMANDS.iter().enumerate() {
        let name = names
            .get(&i.cmd)
            .map_or(i.name, String::as_str)
            .to_lowercase();
        let others = format!("{} {} {} {}", i.name, i.group, i.hint, i.words).to_lowercase();
        let words = |t: &str| {
            t.split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        let (name_words, other_words) = (words(&name), words(&others));
        let mut score = 0;
        let mut all = true;
        for t in &typed {
            if name_words.iter().any(|w| w.starts_with(t)) {
                score += 3;
            } else if name.contains(t) {
                score += 2;
            } else if other_words.iter().any(|w| w.starts_with(t)) {
                score += 1;
            } else {
                all = false;
                break;
            }
        }
        if !all {
            continue;
        }
        if !query.is_empty() && name.starts_with(&query) {
            score += 10;
        }
        found.push((score, order, i.cmd));
    }
    found.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    found.into_iter().map(|(_, _, c)| c).collect()
}

/// The command search.
#[derive(Default)]
struct Palette {
    open: bool,
    query: String,
    /// The highlighted row.
    chosen: usize,
    /// Its text field takes the keyboard when it opens.
    focus: bool,
}

/// The key commands window.
#[derive(Default)]
pub(crate) struct Editor {
    open: bool,
    filter: String,
    /// Waiting for a key to give this command.
    pub(crate) capturing: Option<Cmd>,
    /// A key pressed for the first command that the second has.
    clash: Option<(Cmd, Shortcut, Cmd)>,
    note: Option<String>,
}

pub(crate) struct Commands {
    pub(crate) keymap: Keymap,
    /// Where the keys are kept; none in tests.
    path: Option<PathBuf>,
    palette: Palette,
    pub(crate) editor: Editor,
    /// Commands for the timeline to run when it is next drawn.
    pub(crate) timeline: Vec<Cmd>,
    /// Names shown instead of a command's own, such as a layout's.
    pub(crate) names: BTreeMap<Cmd, String>,
}

impl Commands {
    pub(crate) fn new(path: Option<PathBuf>) -> Commands {
        let keymap = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| Keymap::from_text(&t))
            .unwrap_or_default();
        Commands {
            keymap,
            path,
            palette: Palette::default(),
            editor: Editor::default(),
            timeline: Vec::new(),
            names: BTreeMap::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn palette_open(&self) -> bool {
        self.palette.open
    }

    fn save(&mut self) {
        let Some(path) = &self.path else {
            return;
        };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            // Written whole and renamed, so a crash never leaves half a file.
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, self.keymap.to_text())?;
            std::fs::rename(tmp, path)
        };
        if let Err(e) = write() {
            self.editor.note = Some(format!("Couldn't save the keys: {e}"));
        }
    }

    /// Its name as shown: a layout's, say, or its own.
    pub(crate) fn name(&self, cmd: Cmd) -> &str {
        self.names.get(&cmd).map_or(cmd.info().name, String::as_str)
    }

    /// For a hover text: its name and keys, such as "Play or stop (Space)".
    pub(crate) fn hover(&self, cmd: Cmd) -> String {
        let i = cmd.info();
        let what = if i.hint.is_empty() { i.name } else { i.hint };
        match self.keymap.text(cmd) {
            k if k.is_empty() => what.to_string(),
            k => format!("{what} ({k})"),
        }
    }

    /// Takes the pressed keys that run commands out of this frame's
    /// input. While a text field has the keyboard (`typing`), only the
    /// few commands meant for that run; while the search, a menu or a
    /// list is open, none do.
    fn pressed(&mut self, ctx: &egui::Context, typing: bool) -> Vec<Cmd> {
        if let Some(cmd) = self.editor.capturing {
            self.capture(ctx, cmd);
            return Vec::new();
        }
        // The search takes its own keys (Esc closes it).
        if self.palette.open || egui::Popup::is_any_open(ctx) {
            return Vec::new();
        }
        let mut run = Vec::new();
        let keymap = &self.keymap;
        ctx.input_mut(|i| {
            i.events.retain(|e| {
                let Event::Key {
                    key,
                    pressed: true,
                    repeat,
                    modifiers,
                    ..
                } = e
                else {
                    return true;
                };
                let Some(cmd) = keymap.find(Shortcut::pressed(*key, *modifiers)) else {
                    return true;
                };
                let info = cmd.info();
                if typing && !info.typing {
                    return true;
                }
                if !*repeat || info.repeat {
                    run.push(cmd);
                }
                false
            });
        });
        run
    }

    /// The next key pressed goes to `cmd`; Esc stops waiting.
    fn capture(&mut self, ctx: &egui::Context, cmd: Cmd) {
        let pressed = ctx.input_mut(|i| {
            let at = i.events.iter().position(|e| {
                matches!(
                    e,
                    Event::Key {
                        pressed: true,
                        repeat: false,
                        ..
                    }
                )
            })?;
            match i.events.remove(at) {
                Event::Key { key, modifiers, .. } => Some(Shortcut::pressed(key, modifiers)),
                _ => None,
            }
        });
        let Some(s) = pressed else {
            return;
        };
        self.editor.capturing = None;
        if s.key == Key::Escape && s.mods == NONE {
            return;
        }
        if let Some(why) = s.fixed() {
            self.editor.note = Some(format!("{why}, so it can't be given to a command."));
            return;
        }
        match self.keymap.find(s) {
            Some(c) if c == cmd => {}
            Some(other) => self.editor.clash = Some((cmd, s, other)),
            None => {
                self.keymap.assign(cmd, s);
                self.editor.note = Some(format!("{} is now {}.", s.text(), cmd.info().name));
                self.save();
            }
        }
    }

    /// The command search, which gives back the command chosen.
    fn palette_window(
        &mut self,
        ctx: &egui::Context,
        can_run: impl Fn(Cmd) -> bool,
    ) -> Option<Cmd> {
        if !self.palette.open {
            return None;
        }
        let found = search_named(&self.palette.query, &self.names);
        let mut run = None;
        let mut open = true;
        egui::Window::new("Find a command")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 60.0))
            .default_width(420.0)
            .show(ctx, |ui| {
                // Taken before the text field sees them.
                let (up, down, enter, esc) = ui.input_mut(|i| {
                    (
                        i.consume_key(NONE, Key::ArrowUp),
                        i.consume_key(NONE, Key::ArrowDown),
                        i.consume_key(NONE, Key::Enter),
                        i.consume_key(NONE, Key::Escape),
                    )
                });
                let field = ui.add(
                    egui::TextEdit::singleline(&mut self.palette.query)
                        .hint_text("Type to find a command")
                        .desired_width(f32::INFINITY),
                );
                let field = field.accessible_name("Type a command's name");
                if self.palette.focus {
                    field.request_focus();
                    self.palette.focus = false;
                }
                if field.changed() {
                    self.palette.chosen = 0;
                }
                let last = found.len().saturating_sub(1);
                if up {
                    self.palette.chosen = self.palette.chosen.saturating_sub(1);
                }
                if down {
                    self.palette.chosen = (self.palette.chosen + 1).min(last);
                }
                self.palette.chosen = self.palette.chosen.min(last);
                if esc {
                    open = false;
                }
                if found.is_empty() {
                    ui.weak("No command has that name.");
                }
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        for (n, &cmd) in found.iter().enumerate() {
                            let i = cmd.info();
                            let name = self.name(cmd).to_string();
                            let chosen = n == self.palette.chosen;
                            let enabled = can_run(cmd);
                            let row = egui::Button::selectable(
                                chosen,
                                (
                                    name.as_str(),
                                    RichText::new(format!("  {}", i.group)).weak().small(),
                                ),
                            )
                            .right_text(RichText::new(self.keymap.text(cmd)).strong())
                            .min_size(egui::vec2(ui.available_width(), 0.0));
                            let r = ui.add_enabled(enabled, row).accessible_name(&name);
                            if chosen && (up || down) {
                                r.scroll_to_me(None);
                            }
                            if r.clicked() || (chosen && enter && enabled) {
                                run = Some(cmd);
                            }
                        }
                    });
            });
        if run.is_some() || !open {
            self.palette.open = false;
            ctx.request_repaint();
        }
        run
    }

    /// Settings → Key commands: every command's keys, to change.
    fn editor_window(&mut self, ctx: &egui::Context) {
        let mut open = self.editor.open;
        egui::Window::new("Key commands")
            .open(&mut open)
            .default_width(560.0)
            .default_height(520.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.editor.filter)
                            .hint_text("Find a command")
                            .desired_width(240.0),
                    )
                    .accessible_name("Find a command to change");
                    if ui
                        .button("Reset all")
                        .on_hover_text("Every command back to its keys out of the box")
                        .clicked()
                    {
                        self.keymap = Keymap::default();
                        self.editor.note = Some("Every command has its first keys again.".into());
                        self.save();
                    }
                });
                if let Some(cmd) = self.editor.capturing {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!(
                            "Press the key for {}, with Ctrl, Alt or Shift if you like. Esc to stop.",
                            cmd.info().name
                        ),
                    );
                }
                if let Some((cmd, s, other)) = self.editor.clash {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!(
                            "{} is already {}. Move it to {}?",
                            s.text(),
                            other.info().name,
                            cmd.info().name
                        ));
                        if ui.button("Move it").clicked() {
                            self.keymap.assign(cmd, s);
                            self.editor.clash = None;
                            self.editor.note = Some(format!(
                                "{} is now {}, and no longer {}.",
                                s.text(),
                                cmd.info().name,
                                other.info().name
                            ));
                            self.save();
                        }
                        if ui.button("Leave it").clicked() {
                            self.editor.clash = None;
                        }
                    });
                }
                if let Some(n) = &self.editor.note {
                    ui.label(n);
                }
                ui.separator();
                let shown = search_named(&self.editor.filter, &self.names);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    egui::Grid::new("key commands")
                        .striped(true)
                        .min_col_width(60.0)
                        .num_columns(4)
                        .show(ui, |ui| {
                            for i in COMMANDS.iter().filter(|i| shown.contains(&i.cmd)) {
                                self.editor_row(ui, i);
                                ui.end_row();
                            }
                        });
                    ui.add_space(8.0);
                    ui.strong("Keys that can't be changed");
                    egui::Grid::new("fixed keys").striped(true).show(ui, |ui| {
                        for (keys, what) in FIXED {
                            ui.strong(*keys);
                            ui.label(*what);
                            ui.end_row();
                        }
                    });
                    ui.weak("Letters, Space and the like work when no text field has the keyboard.");
                });
            });
        self.editor.open = open;
        if !open {
            self.editor.capturing = None;
            self.editor.clash = None;
        }
    }

    fn editor_row(&mut self, ui: &mut egui::Ui, i: &Info) {
        let r = ui.label(self.name(i.cmd));
        if !i.hint.is_empty() {
            r.on_hover_text(i.hint);
        }
        ui.weak(i.group);
        ui.horizontal(|ui| {
            let keys = self.keymap.of(i.cmd).to_vec();
            if keys.is_empty() {
                ui.weak("no key");
            }
            for s in keys {
                if ui
                    .add(
                        egui::Button::new(format!("{} ✕", s.text()))
                            .wrap_mode(egui::TextWrapMode::Extend),
                    )
                    .accessible_name(format!("Remove {} from {}", s.text(), i.name))
                    .on_hover_text("Remove this key")
                    .clicked()
                {
                    self.keymap.remove(i.cmd, s);
                    self.save();
                }
            }
        });
        ui.horizontal(|ui| {
            let waiting = self.editor.capturing == Some(i.cmd);
            if ui
                .add(egui::Button::selectable(
                    waiting,
                    if waiting { "Press a key…" } else { "Add key" },
                ))
                .accessible_name(format!("Add a key for {}", i.name))
                .clicked()
            {
                self.editor.capturing = (!waiting).then_some(i.cmd);
                self.editor.clash = None;
                self.editor.note = None;
            }
            let default = self.keymap.of(i.cmd) == i.keys;
            if ui
                .add_enabled(!default, egui::Button::new("Reset"))
                .accessible_name(format!("Reset the keys for {}", i.name))
                .on_hover_text(match i.keys {
                    [] => "Back to no key".to_string(),
                    keys => format!(
                        "Back to {}",
                        keys.iter().map(|s| s.text()).collect::<Vec<_>>().join(", ")
                    ),
                })
                .clicked()
            {
                self.keymap.reset(i.cmd);
                self.save();
            }
        });
    }
}

impl App {
    /// Runs the commands whose keys were pressed. Called before
    /// anything is drawn, so they come before the controls see the keys.
    pub(crate) fn command_keys(&mut self, ctx: &egui::Context) {
        // A clip or marker holding the keyboard is not a text field.
        let typing = ctx.egui_wants_keyboard_input() && !self.timeline.has_keyboard(ctx);
        for cmd in self.commands.pressed(ctx, typing) {
            self.run(ctx, cmd);
        }
    }

    /// The command search and key commands windows.
    pub(crate) fn command_windows(&mut self, ctx: &egui::Context) {
        let (can_undo, can_redo, idle) = (
            self.can_undo(),
            self.can_redo(),
            self.proj.bundle_job.is_none(),
        );
        let layouts = self.workspace.saved.len();
        let chosen = self.commands.palette_window(ctx, |c| match c {
            Cmd::Layout(n) => (n as usize) < layouts,
            Cmd::Undo => can_undo,
            Cmd::Redo => can_redo,
            Cmd::ExportBundle | Cmd::ImportBundle => idle,
            _ => true,
        });
        if let Some(cmd) = chosen {
            self.run(ctx, cmd);
        }
        self.commands.editor_window(ctx);
    }

    fn can_run(&self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::Layout(n) => (n as usize) < self.workspace.saved.len(),
            Cmd::Undo => self.can_undo(),
            Cmd::Redo => self.can_redo(),
            Cmd::ExportBundle | Cmd::ImportBundle => self.proj.bundle_job.is_none(),
            Cmd::MakePatternUnique | Cmd::PatternToNotes => self.selected_pattern_clip().is_some(),
            Cmd::KitNext | Cmd::KitSkip | Cmd::KitBack | Cmd::KitCancel => self.kits.learning(),
            _ => true,
        }
    }

    /// A menu entry for the command, with its keys beside it.
    pub(crate) fn menu_item(&mut self, ui: &mut egui::Ui, cmd: Cmd) {
        let i = cmd.info();
        let keys = self.commands.keymap.text(cmd);
        let mut r = ui.add_enabled(
            self.can_run(cmd),
            egui::Button::new(self.commands.name(cmd)).shortcut_text(keys),
        );
        if !i.hint.is_empty() {
            r = r.on_hover_text(i.hint);
        }
        if r.clicked() {
            ui.close();
            let ctx = ui.ctx().clone();
            self.run(&ctx, cmd);
        }
    }

    pub(crate) fn run(&mut self, ctx: &egui::Context, cmd: Cmd) {
        if !self.can_run(cmd) {
            return;
        }
        if cmd.on_timeline() {
            self.commands.timeline.push(cmd);
            ctx.request_repaint();
            return;
        }
        // Keys pressed in a pane's own window come while the workspace
        // is being drawn: its commands wait until it has been.
        if self.drawing_panes && cmd.on_workspace() {
            self.workspace_cmds.push(cmd);
            ctx.request_repaint();
            return;
        }
        match cmd {
            Cmd::NewProject => self.then(ctx, After::New),
            Cmd::NewFromTemplate => self.choose_template(),
            Cmd::SaveAsTemplate => self.template_ui.saving = Some(String::new()),
            Cmd::OpenProject => {
                self.proj.dialog = Some(Dialog::Open {
                    found: find_projects(),
                });
            }
            Cmd::StartScreen => self.show_start(),
            Cmd::Save => self.save_or_ask(ctx),
            Cmd::SaveAs => {
                self.proj.dialog = Some(Dialog::SaveAs {
                    name: String::new(),
                    then: None,
                });
            }
            Cmd::ExportBundle => self.export_bundle(ctx),
            Cmd::ImportBundle => self.pick_bundle(),
            Cmd::Snapshots => self.snapshot_ui.open = true,
            Cmd::People => self.people.open = true,
            Cmd::Export => self.export.open = true,
            Cmd::Undo => self.undo(ctx),
            Cmd::Redo => self.redo(ctx),
            Cmd::PlayStop => self.toggle_play(),
            Cmd::ToStart => self.send(Command::Seek(Frames(0))),
            Cmd::Record => {
                self.recording = !self.recording;
                self.send(Command::SetRecording(self.recording));
            }
            Cmd::Loop => {
                self.looping = !self.looping;
                self.send_loop();
            }
            Cmd::Metronome => {
                self.metronome.enabled = !self.metronome.enabled;
                self.send(Command::SetMetronome(self.metronome));
            }
            Cmd::CountIn => self.count_in = !self.count_in,
            Cmd::Mixer => self.workspace.toggle(Pane::Mixer),
            Cmd::MonitorSection => self.workspace.toggle(Pane::Monitor),
            Cmd::Talkback => self.toggle_talkback(),
            Cmd::Dim => self.toggle_room(|c| c.dim = !c.dim),
            Cmd::Mono => self.toggle_room(|c| c.mono = !c.mono),
            Cmd::TakeLanes => self.workspace.toggle(Pane::Takes),
            Cmd::Lessons => self.workspace.toggle(Pane::Lessons),
            Cmd::NotesEditor => self.workspace.toggle(Pane::NotesEditor),
            Cmd::QuantiseNotes
            | Cmd::HumaniseNotes
            | Cmd::LegatoNotes
            | Cmd::FitNotesToScale
            | Cmd::ScaleMode
            | Cmd::SelectNotes
            | Cmd::GrooveFromNotes
            | Cmd::GrooveFromAudio
            | Cmd::ApplyGroove
            | Cmd::MidiLearn => {
                self.run_note_tool(cmd);
            }
            Cmd::PatternEditor => self.workspace.toggle(Pane::Patterns),
            Cmd::AddPattern => self.add_pattern_at_playhead(),
            Cmd::MakePatternUnique => {
                if let Some(key) = self.selected_pattern_clip() {
                    self.make_pattern_unique(&key);
                }
            }
            Cmd::PatternToNotes => {
                if let Some(key) = self.selected_pattern_clip() {
                    self.pattern_to_notes(&key);
                }
            }
            Cmd::Instrument | Cmd::AddLayer | Cmd::SingleInstrument => {
                let Some(ti) = self.layer_track() else {
                    self.message = Some("Add an instrument track first.".into());
                    return;
                };
                self.instrument_open = Some(ti);
                match cmd {
                    Cmd::AddLayer => {
                        self.add_layer(ti);
                    }
                    Cmd::SingleInstrument => {
                        self.single_instrument(ti);
                    }
                    _ => {}
                }
            }
            Cmd::AddInstrumentTrack => {
                let n = self.tracks.iter().filter(|t| t.kind.plays_notes()).count() + 1;
                let t = crate::Track::instrument(format!("Instrument {n}"));
                if let Some(i) = &t.instrument {
                    self.pending_instruments.push((i.id, i.make(self.rate)));
                }
                self.tracks.push(t);
                self.rebuild();
            }
            Cmd::AddInstrument(k) => {
                if let Some(kind) = crate::instrument::kind_at(k) {
                    self.add_instrument_track(kind);
                }
            }
            Cmd::InstrumentWindow => match self.instrument_track(None) {
                Some(i) => self.instrument_open = Some(i),
                None => self.message = Some("There is no instrument track yet.".into()),
            },
            Cmd::SliceNotes => {
                if let Err(e) = self.slice_notes() {
                    self.message = Some(e);
                }
            }
            Cmd::PadBuses => {
                if let Err(e) = self.pad_buses() {
                    self.message = Some(e);
                }
            }
            Cmd::KeepNotes => {
                if self.keep_notes() {
                    self.rebuild();
                } else {
                    self.message = Some(
                        "Nothing to keep: play an instrument track from a MIDI keyboard first."
                            .into(),
                    );
                }
            }
            Cmd::ImportMidi => self.pick_midi_file(ctx),
            Cmd::ExportMidi => self.export_midi_file(),
            Cmd::AutomationLanes => self.workspace.toggle(Pane::Automation),
            Cmd::Browser => self.workspace.toggle(Pane::Browser),
            Cmd::FullSize => self.workspace.toggle_full(),
            Cmd::Layout(n) => self.workspace.recall(n as usize),
            Cmd::SaveLayout => self.workspace.saving = Some(String::new()),
            Cmd::PaneWindows => {
                let on = !self.workspace.windows_allowed;
                self.workspace.allow_windows(on);
            }
            Cmd::Tuner => {
                self.tuner.open = true;
                let c = self.tuner.command();
                self.send(c);
            }
            Cmd::Meters => self.meters.show(None),
            Cmd::LessonNotes => self.notes_ui.open = true,
            Cmd::Comments => self.comment_ui.open = true,
            Cmd::AudioDevice => self.show_devices = true,
            Cmd::Plugins => self.effects.show_manager = true,
            Cmd::PerformanceCheck => self.perf.open = true,
            Cmd::DrumKits
            | Cmd::LearnKit
            | Cmd::KitNext
            | Cmd::KitSkip
            | Cmd::KitBack
            | Cmd::KitCancel
            | Cmd::ImportKit
            | Cmd::ExportKit => self.kit_command(cmd),
            Cmd::KeyCommands => self.commands.editor.open = true,
            Cmd::Theme => self.themes.open = true,
            Cmd::EditGroups => self.groups_ui.open = true,
            Cmd::FixTiming => self.timing.open = true,
            Cmd::HitPoint => {
                let at = self.ctl.status().position().0;
                self.add_hit_point(at);
            }
            Cmd::HitPoints => self.hits_ui.open = true,
            Cmd::Timecode => self.timecode.shown = !self.timecode.shown,
            Cmd::Copy => self.copy_clips(),
            Cmd::Paste => {
                let at = self.ctl.status().position().0;
                if self.paste_clips(None, at) {
                    self.rebuild();
                }
            }
            Cmd::Bounce => self.bounce_selection(),
            Cmd::NextTheme => self.themes.next(),
            Cmd::LightDark => self.themes.flip(ctx),
            Cmd::FindCommand => {
                let p = &mut self.commands.palette;
                p.open = true;
                p.query.clear();
                p.chosen = 0;
                p.focus = true;
            }
            Cmd::CheckUpdates => self.updates.open = true,
            Cmd::CrashReports => self.crashes.open = true,
            Cmd::SoundCredits => self.effects.show_credits = true,
            Cmd::Split
            | Cmd::Delete
            | Cmd::Marker
            | Cmd::ClearSelection
            | Cmd::Crossfade
            | Cmd::ClipFades
            | Cmd::ZoomIn
            | Cmd::ZoomOut
            | Cmd::ZoomSelection
            | Cmd::Snap
            | Cmd::Ripple
            | Cmd::VolumeLines
            | Cmd::GroupEdit => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_every_command_is_listed_once() {
        for (n, i) in COMMANDS.iter().enumerate() {
            assert!(
                COMMANDS[n + 1..]
                    .iter()
                    .all(|j| j.id != i.id && j.cmd != i.cmd),
                "{} twice",
                i.id
            );
            assert_eq!(Cmd::from_id(i.id), Some(i.cmd));
        }
    }

    #[test]
    fn no_two_commands_share_a_key_out_of_the_box() {
        let mut seen = Vec::new();
        for i in COMMANDS {
            for s in i.keys {
                assert!(s.fixed().is_none(), "{} is fixed", s.text());
                assert!(!seen.contains(s), "{} twice", s.text());
                seen.push(*s);
            }
        }
    }

    #[test]
    fn keys_are_written_and_read_back() {
        for text in [
            "Ctrl+Shift+Z",
            "Space",
            "Alt+F5",
            "Ctrl+Alt+Shift+Delete",
            "Home",
            "Ctrl+K",
        ] {
            let s = Shortcut::parse(text).unwrap();
            assert_eq!(Shortcut::parse(&s.text()), Some(s));
        }
        assert_eq!(
            Shortcut::parse("ctrl + shift + z"),
            Some(key(CTRL_SHIFT, Key::Z))
        );
        assert_eq!(Shortcut::parse("Hyper+Z"), None);
        assert_eq!(Shortcut::parse("Ctrl+"), None);
    }

    #[test]
    fn only_changed_keys_are_kept() {
        let mut map = Keymap::default();
        assert_eq!(map.to_text(), "");
        map.assign(Cmd::Loop, key(Modifiers::SHIFT, Key::L));
        map.remove(Cmd::Metronome, key(NONE, Key::K));
        let text = map.to_text();
        assert_eq!(text, "loop L, Shift+L\nmetronome none\n");
        assert_eq!(Keymap::from_text(&text), map);
        // Lines it doesn't know, and keys that can't be given, are passed over.
        let read = Keymap::from_text("tempo-tap T\nloop Tab, F6\n");
        assert_eq!(read.of(Cmd::Loop), &[key(NONE, Key::F6)]);
    }

    #[test]
    fn a_key_given_to_another_command_moves() {
        let mut map = Keymap::default();
        let s = key(NONE, Key::S);
        assert_eq!(map.find(s), Some(Cmd::Split));
        map.assign(Cmd::Save, s);
        assert_eq!(map.find(s), Some(Cmd::Save));
        assert!(map.of(Cmd::Split).is_empty());
        assert_eq!(map.of(Cmd::Save), &[key(CTRL, Key::S), s]);
        // Resetting Split takes S back.
        map.reset(Cmd::Split);
        assert_eq!(map.find(s), Some(Cmd::Split));
        assert_eq!(map.of(Cmd::Save), &[key(CTRL, Key::S)]);
    }

    #[test]
    fn pressed_keys_count_ctrl_and_cmd_as_one() {
        let ctrl = Modifiers { ctrl: true, ..NONE };
        let mac = Modifiers {
            mac_cmd: true,
            command: true,
            ..NONE
        };
        let map = Keymap::default();
        for m in [ctrl, mac, Modifiers::COMMAND] {
            assert_eq!(map.find(Shortcut::pressed(Key::S, m)), Some(Cmd::Save));
        }
        // Shift makes it another key.
        assert_eq!(map.find(Shortcut::pressed(Key::S, Modifiers::SHIFT)), None);
    }

    #[test]
    fn search_finds_by_the_start_of_words_and_other_words() {
        assert_eq!(search("metro")[0], Cmd::Metronome);
        assert_eq!(search("click")[0], Cmd::Metronome);
        assert_eq!(search("save")[..2], [Cmd::Save, Cmd::SaveAs]);
        assert_eq!(search("save as")[0], Cmd::SaveAs);
        assert_eq!(search("LOOP")[0], Cmd::Loop);
        assert_eq!(search("bundle"), [Cmd::ExportBundle, Cmd::ImportBundle]);
        assert_eq!(search("shortcuts")[0], Cmd::KeyCommands);
        assert!(search("zzz").is_empty());
        assert_eq!(search("").len(), COMMANDS.len());
        assert_eq!(search(" ")[0], Cmd::NewProject);
    }
}
