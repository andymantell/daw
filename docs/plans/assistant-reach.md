# Every control an assistant can reach

Every control in the app carries an `// ai:` mark saying how an AI assistant does the same thing (see `crates/app/src/widget_check.rs` and [AI_ASSISTANTS.md](../AI_ASSISTANTS.md)). Controls drawn before that rule were let off and counted in `crates/app/widgets-unmarked.txt`. On 2026-11-02 that list held **623** controls. This plan sorts them and closes them out until the list is empty.

## What the survey found

Each unmarked control was read in the source and followed to what it does.

| Group | Controls | What it needs |
|---|---|---|
| Covered by a command or tool that already exists | 373 | Just a mark (naming the tool's argument where there is one) |
| Only changes what is shown | 103 | Just a `view` mark |
| Only a person may do it | 28 | A `person` mark (see below) |
| A tool covers the area but not this setting | 27 | A new argument or value on that tool |
| Nothing reaches it | 92 | A new tool or command |
| **Total** | **623** | |

### Person-only controls

Some controls should never be an assistant's to use: letting an assistant connect and choosing what it may do (assistants.rs), the person's own name and letting other people into a project or setting their role (people.rs), throwing away unsaved work, agreeing to the daily update check, pairing a phone, and copying a crash report out of the app. These get `// ai: person`, which the check accepts like `view`.

### A mark now names the argument

A mark used to name only a tool, so a control could be marked with a tool that couldn't actually do what the control does (the metronome's higher first-beat click was marked `set_practice`, which had no argument for it). A mark can now name the argument too, for example `// ai: set_practice.metronome.level_db`, and the check fails unless that argument is in the tool's schema. New marks from this work name the argument where the tool has one.

## By file

| File | Covered | View | Person | Partial | Gap |
|---|---|---|---|---|---|
| assistants.rs | 1 | 0 | 11 | 0 | 0 |
| audio_setup.rs | 6 | 0 | 0 | 5 | 0 |
| automation.rs | 6 | 1 | 0 | 0 | 0 |
| browser.rs | 1 | 6 | 0 | 0 | 12 |
| commands.rs | 1 | 4 | 0 | 0 | 4 |
| comments.rs | 11 | 4 | 0 | 1 | 5 |
| comping.rs | 10 | 1 | 0 | 0 | 0 |
| crash.rs | 0 | 6 | 1 | 0 | 0 |
| export.rs | 14 | 1 | 0 | 2 | 1 |
| fretboard.rs | 5 | 6 | 0 | 0 | 0 |
| fx.rs | 0 | 0 | 0 | 0 | 2 |
| groups.rs | 5 | 0 | 0 | 0 | 0 |
| history.rs | 2 | 3 | 0 | 0 | 0 |
| hitpoints.rs | 3 | 10 | 0 | 2 | 7 |
| import_project.rs | 29 | 0 | 0 | 0 | 0 |
| instrument.rs | 25 | 3 | 0 | 3 | 4 |
| interchange.rs | 1 | 2 | 0 | 0 | 0 |
| kits.rs | 4 | 0 | 0 | 1 | 3 |
| latency.rs | 0 | 0 | 0 | 0 | 3 |
| layers.rs | 13 | 1 | 0 | 0 | 0 |
| main.rs | 7 | 1 | 0 | 2 | 16 |
| meters.rs | 0 | 4 | 0 | 0 | 0 |
| mixer.rs | 0 | 0 | 0 | 1 | 0 |
| notation.rs | 6 | 0 | 0 | 0 | 2 |
| note_tools.rs | 25 | 1 | 0 | 0 | 0 |
| notefx.rs | 6 | 0 | 0 | 1 | 2 |
| notes_editor.rs | 14 | 5 | 0 | 0 | 0 |
| patterns.rs | 25 | 1 | 0 | 0 | 0 |
| people.rs | 0 | 0 | 8 | 0 | 0 |
| perf.rs | 1 | 1 | 0 | 0 | 1 |
| presets.rs | 13 | 7 | 0 | 0 | 3 |
| project.rs | 3 | 3 | 2 | 3 | 1 |
| recording.rs | 2 | 0 | 0 | 0 | 7 |
| remote.rs | 0 | 0 | 2 | 0 | 2 |
| sections.rs | 13 | 0 | 0 | 1 | 0 |
| snapshots.rs | 8 | 7 | 0 | 0 | 0 |
| start.rs | 3 | 1 | 0 | 0 | 1 |
| stretch.rs | 13 | 0 | 0 | 0 | 0 |
| templates.rs | 2 | 3 | 0 | 0 | 4 |
| tempo.rs | 12 | 0 | 0 | 0 | 0 |
| theme.rs | 1 | 0 | 0 | 1 | 2 |
| timeline.rs | 69 | 5 | 1 | 1 | 1 |
| timing.rs | 6 | 0 | 0 | 0 | 0 |
| tuner.rs | 0 | 1 | 0 | 1 | 0 |
| update.rs | 0 | 1 | 3 | 0 | 1 |
| versions.rs | 3 | 3 | 0 | 2 | 3 |
| video.rs | 3 | 0 | 0 | 0 | 2 |
| workspace.rs | 1 | 11 | 0 | 0 | 3 |

## The order of work

Step 0 (the first PR) adds the `person` mark and argument checking, and marks every covered, view and person control. That leaves 105 in `widgets-unmarked.txt`, all of them partial or gap.

After that, one small PR per area below. Each adds its tools with an end-to-end test through the assistant interface, marks the controls, and lowers the counts in `widgets-unmarked.txt`.

### 1. Hit points and timecode (done)

`edit_timecode` sets whether timecode shows, the frame rate, the start timecode and where it falls, and adds, renames, moves (to seconds, a timecode, or by frames) and removes hit points. `get_project` lists the timecode setup and hit points.

### 2. Sound browser (done)

`sound_browser` adds and removes folders and looks again, searches the folders or the project's audio (by words and tag), sets a file's tags and tempo (or forgets it), previews and stops, and sets at-project-tempo, loop, play-on-click and level.

### 3. Recording options (done)

The top bar work added `set_record_options` and `keep-last`, which reach all of these.

### 4. Comments, versions, templates, presets, saving (done)

`edit_comment` (and `add_comment` with an end or on the loop), `edit_version`, `save_template` (save and delete), `manage_preset` (delete and export), `save_project` with another name or a path (Save as), `open_project` with `recover`, and the `start-screen-at-launch` command. Checking for updates now stays with the person, as agreeing to the checks does.

### 5. Instruments and effects (done)

`edit_effect` and `remove_effect` move, bypass and remove one note effect; `edit_effect` and `edit_instrument` take `restart` for a plugin that stopped, and `get_effects` and `get_instrument` say why it stopped. `edit_instrument` sets note repeat, spreads and removes sampler zones, and gives the loop slicer its loop and cuts it. `set_practice` takes `practice_mute` for any audio or instrument track. A `drum_kits` tool chooses the kit for each MIDI input, picks one to export, and names a kit being learned.

### 6. Audio setup, latency, keys and settings (done)

`edit_keys`, `set_theme`, the `add-test-tracks` command, and the tuner's A in `set_practice`. `set_audio_device` lists and sets everything on the device page; `audio_setup` shows and chooses in the guided setup; `measure_latency` measures the round trip with a loopback cable. Disconnecting and forgetting a phone are marked for a person, as pairing one is: which devices may control the app is the person's to decide.

### 7. Timeline, sections, export, video, notation, layouts (done)

`paste_clips` (onto a track, or as rendered audio); `edit_markers` with `play_order`; `export_mix` with `kbps` and `manage_exports` (cancel, take one out of the queue); `edit_video` (move, remove; `get_project` lists videos); `set_notation` (view, tuning, capo, staff above and rhythm under the tab; these two had been let through by the capo slider's mark above them); `layouts` (save, remove).

## Where it ended

On 2026-11-03 `widgets-unmarked.txt` holds no controls: every control in the app says how an assistant reaches it, or that it only changes what is shown, or that only a person may use it.

A mark covers the three lines above a control, so a new control right under a marked one can pass the check on its neighbour's mark. Review new controls with that in mind.
