// SPDX-License-Identifier: GPL-3.0-or-later
//! The menu bar, grouped by task (UI polish plan, step one): File, Edit,
//! Track, Clip, Notes, Song, View, Practice and Help. What each holds is
//! the table `MENUS`, so the menus, the command table's `group` (which
//! the command search and the assistant interface show as its menu) and
//! the simpler mode's More submenus all agree; a test checks that.
//!
//! Settings is one window (`settings.rs`), opened from File → Settings…;
//! the commands that open its sections are in no menu of their own.

use eframe::egui;

use crate::App;
use crate::commands::{Cmd, FRETBOARD_MENU};

/// One entry in a menu.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Item {
    /// A command, with its keys beside it.
    Run(Cmd),
    /// A separator, drawn only between entries that are shown.
    Gap,
    /// A pane or panel: ticked while it is open, and clicking closes it.
    Pane(Cmd),
    /// A switch drawn as a checkbox.
    Check(Cmd),
    /// A submenu of commands.
    Sub(&'static str, &'static [Cmd]),
    /// Suggest edits, or Stop suggesting while suggesting.
    Suggest,
    /// The saved layouts, and Save layout….
    Layouts,
}

use Item::{Check, Gap, Pane, Run, Sub};

/// The commands of the Add instrument track submenu, in the command
/// table's order.
const ADD_INSTRUMENT: &[Cmd] = &[
    Cmd::AddInstrument(0),
    Cmd::AddInstrument(5),
    Cmd::AddInstrument(1),
    Cmd::AddInstrument(2),
    Cmd::AddInstrument(3),
    Cmd::AddInstrument(4),
    Cmd::AddInstrument(6),
    Cmd::AddInstrument(7),
    Cmd::AddInstrument(8),
    Cmd::AddInstrument(9),
    Cmd::AddInstrument(10),
    Cmd::AddInstrument(11),
    Cmd::AddInstrument(12),
    Cmd::AddInstrument(13),
];

const NOTE_EFFECTS: &[Cmd] = &[
    Cmd::AddNoteFx(0),
    Cmd::AddNoteFx(1),
    Cmd::AddNoteFx(2),
    Cmd::AddNoteFx(3),
    Cmd::AddNoteFx(4),
    Cmd::AddNoteFx(5),
];

/// Every menu on the menu bar, in order, with what it holds.
pub(crate) const MENUS: &[(&str, &[Item])] = &[
    (
        "File",
        &[
            Run(Cmd::NewProject),
            Run(Cmd::NewFromTemplate),
            Run(Cmd::OpenProject),
            Run(Cmd::StartScreen),
            Gap,
            Run(Cmd::Save),
            Run(Cmd::SaveAs),
            Run(Cmd::SaveAsTemplate),
            Run(Cmd::Snapshots),
            Gap,
            Run(Cmd::Export),
            Sub(
                "Export more",
                &[
                    Cmd::ExportSections,
                    Cmd::ExportThisSection,
                    Cmd::ExportStemPackage,
                    Cmd::ExportParts,
                    Cmd::ExportVideo,
                    Cmd::ExportDawProject,
                    Cmd::ExportMidi,
                    Cmd::ClearExportQueue,
                ],
            ),
            Sub(
                "Import",
                &[
                    Cmd::ImportFromProject,
                    Cmd::ImportDawProject,
                    Cmd::ImportMidi,
                    Cmd::ImportMusicXml,
                ],
            ),
            Gap,
            Run(Cmd::ExportBundle),
            Run(Cmd::ImportBundle),
            Run(Cmd::People),
            Gap,
            Run(Cmd::Settings),
        ],
    ),
    (
        "Edit",
        &[
            Run(Cmd::Undo),
            Run(Cmd::Redo),
            Run(Cmd::History),
            Run(Cmd::Activity),
            Gap,
            Run(Cmd::Copy),
            Run(Cmd::Paste),
            Run(Cmd::Delete),
            Run(Cmd::ClearSelection),
            Gap,
            Item::Suggest,
            Run(Cmd::SuggestedEdits),
            Gap,
            Run(Cmd::Snap),
            Run(Cmd::Ripple),
            Run(Cmd::VolumeLines),
            Run(Cmd::GroupEdit),
            Run(Cmd::EditGroups),
        ],
    ),
    (
        "Track",
        &[
            Run(Cmd::AddAudioTrack),
            Run(Cmd::AddInstrumentTrack),
            Sub("Add a built-in instrument", ADD_INSTRUMENT),
            Run(Cmd::AddBus),
            Run(Cmd::AddFolder),
            Run(Cmd::AddControlGroup),
            Gap,
            Run(Cmd::Instrument),
            Run(Cmd::AddLayer),
            Run(Cmd::SingleInstrument),
            Sub(
                "Instrument tools",
                &[
                    Cmd::LoadSfz,
                    Cmd::UnloadSfz,
                    Cmd::NormaliseZones,
                    Cmd::LoadBank,
                    Cmd::SliceNotes,
                    Cmd::PadBuses,
                    Cmd::InstrumentOutputBuses,
                ],
            ),
            Gap,
            Sub(
                "Versions",
                &[
                    Cmd::TrackVersions,
                    Cmd::DuplicateVersion,
                    Cmd::NewVersion,
                    Cmd::NextVersion,
                    Cmd::PrevVersion,
                    Cmd::DeleteVersion,
                ],
            ),
            Sub(
                "Effect chains",
                &[
                    Cmd::NextChain,
                    Cmd::HearChain(0),
                    Cmd::HearChain(1),
                    Cmd::HearChain(2),
                    Cmd::HearChain(3),
                ],
            ),
            Gap,
            Run(Cmd::Presets),
            Run(Cmd::SaveTrackPreset),
            Run(Cmd::SaveChainPreset),
            Run(Cmd::ImportPreset),
        ],
    ),
    (
        "Clip",
        &[
            Run(Cmd::Split),
            Run(Cmd::ClipFades),
            Run(Cmd::Crossfade),
            Run(Cmd::UnlinkClips),
            Run(Cmd::Bounce),
            Gap,
            Run(Cmd::FixTiming),
            Sub(
                "Tempo and stretch",
                &[
                    Cmd::FollowTempo,
                    Cmd::ClipTempo,
                    Cmd::StretchMarkersFromAttacks,
                    Cmd::AddStretchMarker,
                    Cmd::ClearStretchMarkers,
                    Cmd::QuantiseAudio,
                    Cmd::QuantiseAudioApply,
                    Cmd::TempoFromClip,
                    Cmd::TempoFromClipApply,
                    Cmd::BeatGrid,
                    Cmd::AnchorToBeat,
                    Cmd::AnchorToBar,
                ],
            ),
            Sub(
                "Transpose",
                &[
                    Cmd::TransposeClips,
                    Cmd::TransposeUp,
                    Cmd::TransposeDown,
                    Cmd::ResetTranspose,
                    Cmd::MoveFormants,
                ],
            ),
            Sub(
                "Pitch",
                &[
                    Cmd::FindPitchNotes,
                    Cmd::FindLyrics,
                    Cmd::PitchNotesUp,
                    Cmd::PitchNotesDown,
                    Cmd::SnapPitchNotes,
                    Cmd::SnapPitchNotesToKey,
                    Cmd::FlattenPitchDrift,
                    Cmd::ResetPitchNotes,
                ],
            ),
            Gap,
            Run(Cmd::AddPattern),
            Run(Cmd::MakePatternUnique),
            Run(Cmd::PatternToNotes),
        ],
    ),
    (
        "Notes",
        &[
            Run(Cmd::QuantiseNotes),
            Run(Cmd::HumaniseNotes),
            Run(Cmd::LegatoNotes),
            Run(Cmd::FitNotesToScale),
            Run(Cmd::ScaleMode),
            Run(Cmd::SelectNotes),
            Gap,
            Run(Cmd::GrooveFromNotes),
            Run(Cmd::GrooveFromAudio),
            Run(Cmd::ApplyGroove),
            Gap,
            Sub("Add a note effect", NOTE_EFFECTS),
            Run(Cmd::NoteFxOnOff),
            Run(Cmd::RenderNoteFx),
            Run(Cmd::ClearNoteFx),
            Run(Cmd::NoteRepeat),
            Gap,
            Run(Cmd::KeepNotes),
            Run(Cmd::MidiLearn),
            Gap,
            Run(Cmd::NotationLane),
            Run(Cmd::NotationView),
            Run(Cmd::NotationSettings),
            Sub(
                "Tab",
                &[
                    Cmd::TabEntry,
                    Cmd::StringUp,
                    Cmd::StringDown,
                    Cmd::AssignFrets,
                ],
            ),
            Sub(
                "Export notation",
                &[Cmd::ExportMusicXml, Cmd::ExportTextTab, Cmd::PrintNotation],
            ),
        ],
    ),
    (
        "Song",
        &[
            Run(Cmd::PlayStop),
            Run(Cmd::ToStart),
            Run(Cmd::Record),
            Run(Cmd::Loop),
            Run(Cmd::Metronome),
            Run(Cmd::CountIn),
            Sub("Monitoring", &[Cmd::Talkback, Cmd::Dim, Cmd::Mono]),
            Gap,
            Run(Cmd::Marker),
            Run(Cmd::Sections),
            Run(Cmd::LoopSection),
            Run(Cmd::DuplicateSection),
            Run(Cmd::DeleteSection),
            Run(Cmd::SectionEarlier),
            Run(Cmd::SectionLater),
            Gap,
            Run(Cmd::AddToPlayOrder),
            Run(Cmd::PlayOrder),
            Run(Cmd::WriteOrder),
            Run(Cmd::ClearPlayOrder),
            Gap,
            Run(Cmd::InsertBars),
            Run(Cmd::DeleteBars),
            Run(Cmd::TimeWindow),
            Gap,
            Sub(
                "Hit points and timecode",
                &[
                    Cmd::HitPoint,
                    Cmd::HitPoints,
                    Cmd::TimecodeFromFilm,
                    Cmd::PrevHitPoint,
                    Cmd::NextHitPoint,
                    Cmd::FrameBack,
                    Cmd::FrameForward,
                ],
            ),
        ],
    ),
    (
        "View",
        &[
            Pane(Cmd::Mixer),
            Pane(Cmd::MonitorSection),
            Pane(Cmd::TakeLanes),
            Pane(Cmd::NotesEditor),
            Pane(Cmd::PatternEditor),
            Pane(Cmd::PitchEditor),
            Pane(Cmd::AutomationLanes),
            Pane(Cmd::Browser),
            Pane(Cmd::Video),
            Run(Cmd::InstrumentWindow),
            Run(Cmd::Meters),
            Run(Cmd::FullSize),
            Gap,
            Run(Cmd::ZoomIn),
            Run(Cmd::ZoomOut),
            Run(Cmd::ZoomSelection),
            Gap,
            Check(Cmd::Timecode),
            Run(Cmd::VideoTimecode),
            Run(Cmd::ContinuousScroll),
            Run(Cmd::MixerNarrow),
            Gap,
            Item::Layouts,
            Check(Cmd::PaneWindows),
            Gap,
            Check(Cmd::SimpleMode),
            Run(Cmd::Theme),
            Run(Cmd::NextTheme),
            Run(Cmd::LightDark),
        ],
    ),
    (
        "Practice",
        &[
            Pane(Cmd::Tuner),
            Pane(Cmd::Lessons),
            Pane(Cmd::LessonNotes),
            Pane(Cmd::Comments),
            Pane(Cmd::Fretboard),
            Sub("Fretboard tools", &FRETBOARD_MENU),
            Gap,
            Run(Cmd::SlowerSpeed),
            Run(Cmd::FasterSpeed),
            Run(Cmd::NormalSpeed),
            Gap,
            Run(Cmd::PracticeMutePart),
        ],
    ),
    (
        "Help",
        &[
            Run(Cmd::FindCommand),
            Run(Cmd::KeyCommands),
            Run(Cmd::HearValue),
            Gap,
            Run(Cmd::CheckUpdates),
            Run(Cmd::CrashReports),
            Run(Cmd::SoundCredits),
        ],
    ),
];

/// The group of the commands that open a part of the Settings window.
/// They are in no menu: File → Settings… opens the window.
pub(crate) const SETTINGS: &str = "Settings";

/// The menu `cmd` is in, if it is in one.
pub(crate) fn menu_of(cmd: Cmd) -> Option<&'static str> {
    MENUS.iter().find_map(|(title, items)| {
        items
            .iter()
            .any(|i| match *i {
                Run(c) | Pane(c) | Check(c) => c == cmd,
                Sub(_, cs) => cs.contains(&cmd),
                Item::Suggest => matches!(cmd, Cmd::SuggestEdits | Cmd::StopSuggesting),
                Item::Layouts => matches!(cmd, Cmd::Layout(_) | Cmd::SaveLayout),
                Gap => false,
            })
            .then_some(*title)
    })
}

impl App {
    /// Every menu on the menu bar.
    pub(crate) fn menu_bar(&mut self, ui: &mut egui::Ui) {
        for (title, items) in MENUS {
            self.top_menu(ui, title, |app, ui| {
                for item in *items {
                    app.menu_entry_of(ui, *item);
                }
            });
        }
    }

    fn menu_entry_of(&mut self, ui: &mut egui::Ui, item: Item) {
        match item {
            Run(c) => self.menu_item(ui, c),
            Gap => self.menu_gap(),
            Pane(c) => self.pane_entry(ui, c),
            Check(c) => self.check_entry(ui, c),
            Sub(title, cmds) => self.sub_menu(ui, title, cmds),
            Item::Suggest => match self.suggesting() {
                None => self.menu_item(ui, Cmd::SuggestEdits),
                Some(_) => self.menu_item(ui, Cmd::StopSuggesting),
            },
            Item::Layouts => {
                for n in 0..self.workspace.saved.len() {
                    self.menu_item(ui, Cmd::Layout(n as u8));
                }
                self.menu_item(ui, Cmd::SaveLayout);
            }
        }
    }

    /// A submenu of commands. In the simpler mode the ones it leaves out
    /// wait under the menu's More, and a submenu left empty is not drawn.
    pub(crate) fn sub_menu(&mut self, ui: &mut egui::Ui, title: &str, cmds: &[Cmd]) {
        let shown: Vec<Cmd> = cmds
            .iter()
            .copied()
            .filter(|&c| !self.menu_hold(c))
            .collect();
        if shown.is_empty() {
            return;
        }
        self.menu_mark(ui);
        ui.menu_button(title, |ui| {
            for c in shown {
                self.menu_entry(ui, c);
            }
        });
    }

    /// A switch in a menu, ticked while it is on.
    fn check_entry(&mut self, ui: &mut egui::Ui, cmd: Cmd) {
        if self.menu_hold(cmd) {
            return;
        }
        self.menu_mark(ui);
        let mut on = match cmd {
            Cmd::SimpleMode => self.simple.on,
            Cmd::Timecode => self.timecode.shown,
            _ => self.workspace.windows_allowed,
        };
        let hint = match cmd {
            Cmd::PaneWindows => "Off: every pane stays in this window".to_string(),
            _ => self.commands.hover(cmd),
        };
        // ai: simpler-mode
        let r = ui
            .checkbox(&mut on, self.commands.name(cmd).to_string())
            .on_hover_text(hint);
        if r.changed() {
            match cmd {
                Cmd::SimpleMode => self.toggle_simple(),
                Cmd::Timecode => self.timecode.shown = on,
                _ => self.workspace.allow_windows(on),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::COMMANDS;

    /// Every command in a menu is in it once, and its group in the
    /// command table (which the search and assistants show) names it.
    #[test]
    fn the_command_table_names_each_commands_menu() {
        let mut seen = Vec::new();
        for (title, items) in MENUS {
            for item in *items {
                let cmds: Vec<Cmd> = match *item {
                    Run(c) | Pane(c) | Check(c) => vec![c],
                    Sub(_, cs) => cs.to_vec(),
                    Item::Suggest => vec![Cmd::SuggestEdits, Cmd::StopSuggesting],
                    Item::Layouts => vec![Cmd::SaveLayout],
                    Gap => vec![],
                };
                for c in cmds {
                    assert!(!seen.contains(&c), "{c:?} is in two menus");
                    seen.push(c);
                    assert_eq!(c.info().group, *title, "{c:?}");
                }
            }
        }
        let titles: Vec<&str> = MENUS.iter().map(|m| m.0).collect();
        for i in COMMANDS {
            match menu_of(i.cmd) {
                Some(m) => assert_eq!(i.group, m, "{:?}", i.cmd),
                None => assert!(
                    i.group == SETTINGS || titles.contains(&i.group),
                    "{:?} is in the group {}, which is no menu",
                    i.cmd,
                    i.group
                ),
            }
        }
    }

    /// The menus by task, as the UI polish plan lays them out.
    #[test]
    fn the_menus_are_grouped_by_task() {
        let titles: Vec<&str> = MENUS.iter().map(|m| m.0).collect();
        assert_eq!(
            titles,
            [
                "File", "Edit", "Track", "Clip", "Notes", "Song", "View", "Practice", "Help"
            ]
        );
        // No menu is a list of everything: the longest has under 35
        // entries, its submenus counted as one.
        for (title, items) in MENUS {
            let n = items.iter().filter(|i| !matches!(i, Gap)).count();
            assert!(n < 35, "{title} has {n} entries");
        }
    }
}
