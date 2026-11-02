# UI polish, step one: menus by task, one Settings window, right-click menus

Step one of the UI polish plan (`docs/plans/ui-polish.md`, section 4).
Nothing was removed: every command keeps its id, its keys and its place
in the command search (Ctrl+K); only where it sits in the menus changed.

## What changed

**Nine menus, grouped by task**: File, Edit, Track, Clip, Notes, Song,
View, Practice, Help. Before, Edit held 99 commands and Practice repeated
items from View. Now:

| Menu | Holds |
|---|---|
| File | New, open, start screen; save, save as, templates, snapshots; Export… with an *Export more* submenu (practice files, stems, parts, video, other DAWs, MIDI); an *Import* submenu; bundles, people; **Settings…** |
| Edit | Undo, redo, undo list, activity; copy, paste, delete, clear selection; suggest edits; snap, ripple, volume lines, group editing, edit groups |
| Track | Add audio, instrument, bus, folder, control group, and an *Add a built-in instrument* submenu with all fourteen; instrument and layers, *Instrument tools* (SFZ, FM bank, slices, pad buses); *Versions* and *Effect chains* submenus; presets |
| Clip | Split, fades, crossfade, unlink, bounce; fix timing; *Tempo and stretch*, *Transpose* and *Pitch* submenus; drum patterns |
| Notes | Quantise, humanise, legato, scale, select; grooves; note effects and note repeat; keep the notes I just played, MIDI learn; the notation lane, with *Tab* and *Export notation* submenus |
| Song | Play, back to start, record, loop, metronome, count-in, a *Monitoring* submenu; markers and sections; the play order; insert or delete bars; a *Hit points and timecode* submenu |
| View | The panes (mixer, monitor section, takes, editors, automation, browser, video), instrument window, meters, full size; zoom; timecode and scrolling; layouts; simpler mode and theme |
| Practice | Tuner, lessons, lesson notes, comments, fretboard (with its tools as a submenu); slower, faster, normal speed; practice-mute this part |
| Help | Find a command, key commands, hear the value; updates, crash reports, sound credits |

The menus come from one table in `crates/app/src/menus.rs`. The command
table's "menu" for each command (shown in the command search and given
to assistants by `list_commands`) is checked against it by a test, so
they cannot drift apart.

**Settings is one window** with its sections down the side: Audio, MIDI,
Latency, Plugins, Drum kits, Key commands, Theme, AI assistants, Phone
remote, Performance. It opens from **File → Settings…** (Ctrl+,) on the
section last shown, and the old commands (Audio device…, Plugins…,
Measure latency… and the rest) open it on their section. MIDI devices
were at the bottom of the audio device window and are now a section of
their own. The guided audio setup stays a step-by-step window of its
own, opened from the Audio section.

**Right-click menus**:
- A track's **header** (new): its name, Instrument… (instrument tracks),
  Effects…, render in place, freeze, save as preset, versions, and add
  a comment.
- **Notes** in the note editor (new): delete, select all, quantise,
  humanise, legato, fit to the scale, transpose, select notes, grooves.
  It acts on the selected notes; a right-clicked note joins the
  selection first.
- **Clips**: as before, plus Fix timing….
- **The ruler**: as before (markers, tempo and time signature changes,
  hit points, comments), plus Sections…, Insert or delete bars…, Loop
  and Metronome.

## For the AI assistants

Command ids are unchanged. New commands: `settings` (opens the window),
`midi-devices` and `phone-remote` (open those sections; the phone remote
had no command before). `get_selection` now reports `settings`: the
section showing (`audio`, `midi`, `latency`, `plugins`, `drum-kits`,
`key-commands`, `theme`, `assistants`, `phone-remote`, `performance`),
or null while the window is closed. `list_commands` gives each
command's new menu. The end-to-end test
`settings::tests::assistants_open_each_section_and_see_which_is_showing`
opens every section through the assistant interface and checks what
`get_selection` and `list_commands` say.

## Pictures

At 1920 × 1200 (the baseline) and 1440 × 900, in
`ui-polish-1-menus-settings/`:

- `1920-menus.png`: all nine menus open, side by side.
- `1920-settings-audio.png`, `1920-settings-midi.png`,
  `1920-settings-plugins.png`, `1920-settings-key-commands.png`,
  `1920-settings-theme.png`: the Settings window.
- `1920-right-click-clip.png`, `1920-right-click-track.png`,
  `1920-right-click-ruler.png`, `1920-right-click-notes.png`.
- `1440-menu-file.png`, `1440-menu-clip.png`, `1440-settings-audio.png`,
  `1440-right-click-track.png`: the same at the smaller screen.

The icons still draw as empty boxes in these pictures: a separate
thread is drawing our own icon set.

## For you to decide

1. **Play, record, loop and the metronome are in the Song menu.** The
   plan's nine menus have no Transport menu, so these sit at the top of
   Song, with talkback, dim and mono in a *Monitoring* submenu. The top
   bar keeps its own buttons for all of them. Say if you'd rather keep a
   Transport menu.
2. **Right-clicking a note opens a menu instead of deleting it.** Delete
   is the first entry, and the Delete key still deletes the selection.
   Say if you'd rather keep one-click delete on the right button (then
   the menu would open only on an empty place).
3. **In the simpler mode the Track menu is hidden**, because none of its
   commands is marked everyday (as before, when they were in Edit). Say
   if Add audio track and Add instrument track should be everyday.
4. **Key commands and Theme are in two places**: Help → Key commands…
   and View → Theme… open the Settings window on those sections. Fine,
   or only in Settings?
