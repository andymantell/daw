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

### 2. Sound browser (12)

New `sound_browser` tool: list folders, add or remove one, look again; set a file's tags and tempo; preview play and stop with at-project-tempo, loop, play-on-click and level.

- `browser.rs:580` Stop browsing folder (gap)
- `browser.rs:589` Add folder… (gap)
- `browser.rs:598` Look again (gap)
- `browser.rs:629` Play/Stop sound preview (gap)
- `browser.rs:687` At project tempo checkbox (gap)
- `browser.rs:689` Loop preview checkbox (gap)
- `browser.rs:690` Play on click checkbox (gap)
- `browser.rs:693` Preview level slider (gap)
- `browser.rs:713` Stop preview (gap)
- `browser.rs:748` Its tempo (bpm) (gap)
- `browser.rs:761` Forget tempo (gap)
- `browser.rs:778` Its tags field (gap)

### 3. Recording options (done)

The top bar work added `set_record_options` and `keep-last`, which reach all of these.

### 4. Comments, versions, templates, presets, saving (24)

Edit and delete a comment and let one span a range; rename a track version and delete one by id; a `save_template` tool with delete; delete a preset and export one to a file; Save as under a new name or path; restore the recovery file; a command for "show the start screen at launch" and one to check for updates now.

- `presets.rs:904` Delete preset confirm: Delete (gap)
- `presets.rs:949` Export… preset (gap)
- `presets.rs:956` Delete… preset (gap)
- `comments.rs:167` On the loop radio (partial)
- `comments.rs:328` Edit comment text (gap)
- `comments.rs:336` Save edited comment (gap)
- `comments.rs:368` Delete for good (gap)
- `comments.rs:380` Edit comment (gap)
- `comments.rs:387` Delete comment (gap)
- `project.rs:1354` Restore (recovery) (gap)
- `project.rs:1389` Save project name (partial)
- `project.rs:1398` Save (save as) (partial)
- `project.rs:1402` Somewhere else… (partial)
- `start.rs:245` Show this when the app starts (gap)
- `templates.rs:191` Delete (template, confirm) (gap)
- `templates.rs:208` Delete… (template) (gap)
- `templates.rs:253` Template name (gap)
- `templates.rs:269` Save / Replace (template) (gap)
- `update.rs:436` Check now (gap)
- `versions.rs:343` Version name field (gap)
- `versions.rs:350` Rename (version, OK) (gap)
- `versions.rs:367` Delete (version, confirm) (partial)
- `versions.rs:386` Rename (version) (gap)
- `versions.rs:389` Delete… (version) (partial)

### 5. Instruments and effects (17)

Restart a crashed plugin (and report the crash); the loop slicer's loop and how it cuts; remove a sampler zone and spread zones over the keys; note repeat's rate; remove or move one note effect; a `drum_kits` tool (kit per MIDI input, name and factory notes while learning, export by name); practice mute for any track.

- `fx.rs:1715` Restart crashed effect plugin (gap)
- `fx.rs:2226` Restart crashed plugin instrument (gap)
- `instrument.rs:1227` Note repeat: rate (partial)
- `instrument.rs:1558` Sampler: Spread over the keys (partial)
- `instrument.rs:1604` Sampler: Remove zone (partial)
- `instrument.rs:2153` Slicer: Cut at the hits (gap)
- `instrument.rs:2160` Slicer: Sensitivity slider (gap)
- `instrument.rs:2165` Slicer: Cut into equal parts (gap)
- `instrument.rs:2170` Slicer: Parts count (gap)
- `kits.rs:433` Drum kit for MIDI input (gap)
- `kits.rs:457` Kit list entry (partial)
- `kits.rs:507` Kit learning: name field (gap)
- `kits.rs:510` Kit learning: factory notes radio (gap)
- `mixer.rs:581` Mixer strip P (practice mute) (partial)
- `notefx.rs:686` Note effect Remove (partial)
- `notefx.rs:693` Note effect Move down (gap)
- `notefx.rs:700` Note effect Move up (gap)

### 6. Audio setup, latency, keys and settings (37)

A `set_audio_device` tool for the device page (driver, device, rate, buffers, inputs, offset, direct monitoring, click and main outputs, apply, rescan); the guided setup's choices; a `measure_latency` tool; an `edit_keys` tool; theme mode, copy and reload; phone remote disconnect and forget; perf test tracks; the tuner's A reference.

- `main.rs:2258` Audio device: Driver choice (gap)
- `main.rs:2273` Audio device: Device choice (gap)
- `main.rs:2293` Audio device: Sample rate (gap)
- `main.rs:2306` Audio device: Buffer 'Driver default' (gap)
- `main.rs:2312` Audio device: Buffer size choice (gap)
- `main.rs:2326` Audio device: Mix buffer size choice (partial)
- `main.rs:2351` Audio device: Input device 'None' (gap)
- `main.rs:2353` Audio device: Input device choice (gap)
- `main.rs:2364` Recording offset: 'Measured' checkbox (gap)
- `main.rs:2370` Recording offset frames (gap)
- `main.rs:2403` Monitoring: Direct checkbox (gap)
- `main.rs:2411` Metronome output: Main mix (gap)
- `main.rs:2413` Metronome output: Output n (gap)
- `main.rs:2481` Audio device: Apply (gap)
- `main.rs:2491` Audio device: Rescan devices (gap)
- `main.rs:2740` Mixer take picker: Off (partial)
- `main.rs:2753` Audio device: Main output L/R channel (gap)
- `main.rs:2938` Status bar: Cancel render (gap)
- `audio_setup.rs:520` Audio setup: interface choice (radio per device) (partial)
- `audio_setup.rs:534` Audio setup: monitor through the app (radio) (partial)
- `audio_setup.rs:545` Audio setup: monitor through the interface (radio) (partial)
- `audio_setup.rs:580` Audio setup: buffer size (radio per size) (partial)
- `audio_setup.rs:613` Audio setup: input channel combo (partial)
- `commands.rs:3042` Key commands: Reset all (gap)
- `commands.rs:3068` Key commands: Move it (key clash) (gap)
- `commands.rs:3132` Key commands: remove a key (x) (gap)
- `commands.rs:3158` Key commands: Reset (per command) (gap)
- `latency.rs:95` Measure latency: loopback input combo (gap)
- `latency.rs:119` Measure latency: Cancel (gap)
- `latency.rs:126` Measure latency: Start measuring (gap)
- `perf.rs:159` Performance check: Add test tracks (gap)
- `remote.rs:188` Phone remote: Disconnect (connected phone) (gap)
- `remote.rs:205` Phone remote: Forget (paired phone) (gap)
- `theme.rs:988` Theme: Colours mode (Designed/System/Dark/Light) (partial)
- `theme.rs:1008` Theme: Copy to edit (gap)
- `theme.rs:1027` Theme: Reload (gap)
- `tuner.rs:99` Tuner: A = reference Hz (partial)

### 7. Timeline, sections, export, video, notation, layouts (13)

Paste onto a chosen lane and paste as rendered audio; cancel a render; a take picker's Off; set the play order as a list; MP3/Opus bit rate, cancel an export and remove one from the queue; see and move or remove video clips; staff and rhythm under the tab; save and remove a named layout.

- `sections.rs:554` Play order entry 'Name ×' (partial)
- `timeline.rs:4654` Lane menu: Paste here (partial)
- `timeline.rs:4663` Lane menu: Paste here as rendered audio (gap)
- `workspace.rs:1671` Save layout: name field (gap)
- `workspace.rs:1690` Save layout: Save (gap)
- `workspace.rs:1711` Save layout: Remove layout (gap)
- `export.rs:654` Bit rate choice (partial)
- `export.rs:742` Cancel export (gap)
- `export.rs:756` Take it out of the queue (partial)
- `notation.rs:1663` Notation window: Standard staff above the tab (gap)
- `notation.rs:1664` Notation window: Rhythm under the tab (gap)
- `video.rs:478` Video: Move to the start of the song (context menu) (gap)
- `video.rs:494` Video: Remove the picture (context menu) (gap)

Line numbers are from main at affde6a and move as files change.
