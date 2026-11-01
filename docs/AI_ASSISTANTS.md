# AI assistants: connecting one, and keeping features reachable

The app has a server inside it that an AI assistant connects to over the
[Model Context Protocol](https://modelcontextprotocol.io) (MCP). Through
it, an assistant can do what a person does in the app (run any command,
change any setting, edit clips, notes, markers and effects, save and
export) and perceive what a person perceives (the project, the
transport, the meters, analysis of any stretch, rendered audio, and a
picture of any pane). Roadmap 2.11; the design notes are in
[research/AGENT_INTERFACE.md](research/AGENT_INTERFACE.md).

## Connecting an assistant to the app

1. In the app, open **Settings → AI assistants…** and turn the server on.
   It listens only on this computer, at `http://127.0.0.1:7346/mcp` (or
   the next free port), and needs the token shown in that window.
2. Open **Settings to paste into an assistant** and copy the text. It
   looks like this:

   ```json
   {
     "mcpServers": {
       "daw": {
         "type": "http",
         "url": "http://127.0.0.1:7346/mcp",
         "headers": { "Authorization": "Bearer <token>" }
       }
     }
   }
   ```

   Any assistant that takes MCP servers over HTTP with a header can use
   it. For Claude Code, save it as `.mcp.json` in the folder you run it
   from, or pass it with `claude --mcp-config <file>`.
3. The first time the assistant calls, the window asks what it may do:
   **Look only**, **Edit**, **Edit and export** (also save and export),
   or **Refuse**.
4. Every request that changes the project is one undo step, named for
   what the assistant did, so Ctrl+Z takes back one request at a time.
   **Stop assistants** (Ctrl+Shift+Backspace) turns the server off and
   forgets every permission.

## Running the app with no window

```text
daw --headless [--port N] [--access read|edit|export] [file ...]
```

This runs the app with no window and no sound, serving the same
interface. It is for an assistant exercising a build, and for tests that
drive the real program.

- The token comes from the `DAW_AGENT_TOKEN` environment variable, or is
  made up.
- The first line printed is `listening <address>`. When the token was
  made up, the next line gives it.
- There is no one to ask, so every assistant gets the `--access` given
  (`edit` unless given). Use `export` to let it save and export.
- Pictures are refused, as nothing is drawn.
- The window's AI assistant settings are not changed.
- It stops when killed (Ctrl+C).

For example, with Claude Code:

```sh
DAW_AGENT_TOKEN=secret daw --headless --port 7399 --access export &
claude -p "Make a four-bar drum loop at 100 bpm and export it" \
  --mcp-config mcp.json --allowedTools mcp__daw
```

Here `mcp.json` is the snippet above with port 7399 and token `secret`.

## What an assistant can call

The server lists every tool with its settings (`tools/list`), so an
assistant finds them for itself. In outline:

- **Commands:** `list_commands` and `run_command` reach every command in
  the menus, keys and command search, by id. A command that acts on an
  instrument track or a note clip (note effects, layers, the note tools,
  patterns) takes the `track` or `clip` to act on. `file_command` runs
  one that would ask for a file, with the file given instead.
- **Reading:** `get_project`, `get_transport`, `get_selection`,
  `list_parameters`, `get_meters`, `get_history`, `get_comments`,
  `get_fretboard`, `get_exports`, `list_presets`, `list_effect_kinds`,
  `list_projects`, `export_report`, `get_takes`, `list_snapshots`,
  `get_instrument`, `get_effects`. `list_parameters` lists every
  parameter of a plugin, and takes `effect`, `search`, `offset` and
  `limit` to narrow and page them.
- **Hearing and seeing:** `analyse` (peak, true peak, RMS, crest factor,
  loudness and its range, stereo correlation, third-octave bands, and
  the key and tempo when it can tell) of a track, a bus, one clip or the
  mix, over a time range, with or without one track. With `notes` it also
  reads one voice or instrument note by note: each note with its start,
  length and how many cents off it is, the seconds of each of the 12
  notes, the overall tuning and the key they fit. With `beats` it finds
  the beat grid of a voice alone or another sparse part, from its note
  starts, where the attack-based tempo often finds nothing: up to five
  tempo candidates from 60 to 200 bpm, each with its confidence, the
  chance onsets at random would fit as well, the first beat and first
  downbeat in timeline seconds (and the downbeat's place on the song's
  bars), and how far the beat drifts (the timeline's "Find beat grid…"
  window shows the same). `transpose_semitones`
  hears a track or clip as if moved, without changing it, so a shift can
  be tried before it is made. `render_audio` (a WAV file);
  `get_picture` (the window or one pane). `get_project` with `summary`
  gives each track's clips as a count and a span, for a first look at a
  big project. `hear_value` plays a pan, level, setting or automation
  lane to the person as a short sound and says it in words, as Hear the
  value does.
- **Following the person:** `wait_for_change` answers when the project
  next changes, by anyone.
- **Tracks, routing and effects:** `add_track`, `edit_track`,
  `delete_track`, `set_send`, `set_parameter` (a built-in instrument's
  settings too, as `instrument.<key>`), `set_parameters` (many settings,
  on one or many tracks, in one call and one undo step; all or none, so
  an interruption can't leave a voicing half-set), `set_sound` (a sound file on a
  drum pad, a sampler zone or the drum sampler), `edit_control_group`,
  `add_effect`,
  `edit_effect`, `remove_effect`, `add_preset`, `save_preset`,
  `switch_version`. `add_effect` and `edit_effect` place an effect in a
  layer's chain or a container's chain (`container`, `branch`), and
  `edit_effect` wires it: `sidechain`, a plugin's extra `outputs`, and
  the `file` a convolution reverb, speaker cabinet or amp capture
  player uses. `plugin_preset` loads, saves or deletes a plugin's
  presets and picks its programs.
  The master channel (the whole mix's effects and fader, after every
  track and bus) is the track `master` to `add_effect`, `edit_effect`,
  `remove_effect`, `list_parameters`, `set_parameter`, `analyse` and
  the automation tools (`get_automation`, `edit_automation`,
  `set_automation_mode`: its volume and effect settings, no pan), and
  `get_project` shows it as `master`. Exported stems go through it.
- **Instruments and layers:** `set_instrument` (a built-in or plugin
  instrument, for a track or one layer, and/or a `settings` dict for it,
  as `edit_layer` takes for a layer), an SFZ sample library on the
  sampler (`file_command` with `load-sfz`, or `set_sound` with the
  slot `sfz`; `get_instrument` then lists its keys, articulations and
  controls, and each control, such as a mic level, is a setting,
  `instrument.ccN`), `edit_layer` (add, remove,
  reorder, switch on or off, instrument, level, pan, note and velocity
  ranges, transpose, and its instrument's settings), `edit_pad_effect`
  (a drum machine pad's effects) and `set_instrument_output` (an
  instrument's extra outputs to buses).
- **Arranging:** `import_audio`, `edit_clip`, `delete_clip`,
  `add_note_clip`, `edit_notes`, `edit_markers`, `set_tempo`, `set_key`,
  `set_loop`, `seek`, `select_clips`, `undo_steps`, `add_comment`,
  `resolve_comment`. `set_tempo` takes time signature changes too.
  `edit_clip` with `anchor_seconds` moves an audio clip by a point in it
  (timeline seconds, such as a note start or downbeat from `analyse`):
  that point lands on the `bar` or `seconds` given, or with `snap` on the
  nearest beat or bar line.
- **Takes, timing and clip processing:** `edit_comp` (new, duplicate,
  choose, rename, delete, flatten, swipe, clear, use a take, choose a
  take), `fix_timing` (with its grid, strength and sensitivity, or a
  preview), `stretch_clips` (follow tempo, a clip's own tempo, stretch
  markers placed, moved, removed or quantised), `tempo_from_clip`,
  `freeze_track`, `render_track` (render in place, or bounce clips),
  `process_clips` (normalise, reverse, change gain), `edit_clip_volume`
  (fade shapes and the volume line), `edit_groups`, `edit_snapshot`, and
  `set_track_input` (arm, monitor, input and MIDI out, which belong to
  this computer, not the project).
- **Suggesting edits (review mode):** `edit_suggestion` with `start`
  makes every later edit, by any tool, go to a suggestion instead of
  the song, until `stop`; `resume` carries on an earlier one.
  `get_suggestions` lists each suggestion's changes by key, in words,
  with what was decided; `edit_suggestion` with `accept` or `reject`
  takes or turns down changes by key (all open ones if none are given).
  Accepting is one undoable edit.
- **Notes and patterns:** `get_note_clip` (notes with their chance and
  groups, expression, controller lanes, the clip's key), `edit_notes`
  (add, change, remove, group), `edit_note_lanes` (controller lanes and
  per-note expression), `duplicate_clip` (linked or not, as many
  copies as asked, back to back, every so many bars, or one on each of
  a list of `bars` or `at_seconds`), `note_tool`
  (quantise, humanise, legato, fit to scale, apply or make a groove,
  each with its settings), `list_grooves`, `get_patterns` and
  `edit_pattern` (the step sequencer: patterns, lanes and steps).
  `edit_clip` moves, resizes, mutes and keys note clips too, loops
  their notes (`loop`, `loop_beats`; `add_note_clip` takes
  `loop_bars`) so a longer clip plays them again and again, and
  transposes audio clips (`transpose_semitones`, fractions for cents,
  with `move_formants`). An audio clip's `offset_seconds` and
  `len_seconds` are in the file's own seconds; for a clip that follows
  tempo or has stretch markers, `end_seconds` says where it ends on the
  timeline.
  Pitch editing of a single voice or single-note line: `edit_pitch_notes`
  finds an audio clip's notes (`find: true`) and edits them one by one
  or together (`to_midi`, `to_plays_midi`, `shift_semitones`,
  `by_semitones`, `snap`, `move_seconds`, `resize_seconds`, `drift_pct`,
  `vibrato_pct`, `formant_semitones`, `reset`, and `min_cents` with
  `snap`). `snap: "key"` snaps to the song's key instead of the nearest
  semitone, both as heard with the clip's transpose, and `min_cents`
  leaves notes already that close; the answer says how many were
  `snapped`. `get_pitch_notes` reads them back, with each note's pitch
  line when asked (`line: true`).
  Each note gives its pitch in the file (`pitch`, `now`) and as heard
  with the clip's transpose (`plays`, `plays_midi`), and its time in the
  file's seconds (`start_seconds`) and on the timeline
  (`timeline_seconds`, `timeline_end_seconds`).
  Words in a vocal: `find_lyrics` reads the words sung in an audio
  clip's recording with a speech model on the computer (downloaded once,
  only with `download_model: true`), and `get_lyrics` gives them in
  lines, each word with its time from the clip's start, in its file (for
  `edit_clip offset_seconds`), in the song and as a bar, and how sure
  the model is.
  `add_note_clip` and `edit_notes` take chords by name (`Dm9`, `G/B`)
  or several pitches at once, and `select_clips` picks a note clip.
- **Automation:** `get_automation` (each track's mode, lanes and
  points), `edit_automation` (add or remove a lane for any setting
  `list_parameters` names, clear it, and add, move or delete points;
  points are joined by straight lines), `set_automation_mode` (Off,
  Read, Touch, Latch or Write). A built-in instrument's settings
  (`instrument.cutoff` on a synth, say) take lanes too, and the
  `filter` effect (low-pass, high-pass, band-pass or notch, 20 Hz to
  20 kHz, with resonance) is made for sweeps.
- **Lessons:** `get_lessons` (the lesson notes, tab blocks,
  assignments with who has done them, lessons and the practice log),
  `edit_lesson_notes` (the notes' text, and tab blocks pinned to a
  section), `edit_assignments` (set, change, remove, mark done, or
  practise one) and `edit_lessons` (start, rename or remove a lesson,
  or take a practice session out of the log).
- **MIDI learn:** `get_midi_maps` and `edit_midi_maps` (tie a MIDI
  device's controller to any setting, untie it, choose whether each
  press steps the setting on, as a footswitch does, and move a
  controller as if from a device, to try a tie).
- **Switching containers** (one chain heard at a time, such as a clean
  and a driven amp channel): `get_effects` gives the chain `heard`; it
  is the setting `fx:<id>:heard` for `set_parameter`, `edit_automation`
  and `edit_midi_maps`, and the commands `next-chain` and
  `hear-chain-1` to `hear-chain-4` switch it (with `track`).
- **Plugins on this computer:** `get_plugins` and `manage_plugins`
  (rescan, check, try a failed one again, offer or hide, where each
  runs, how many plugin windows may be open).
- **Practice:** `set_practice` (speed, transpose, fine pitch, the
  metronome, count-in, the tempo trainer, and measuring how far a
  track is from concert pitch).
- **Monitor section:** `get_monitor`, `edit_headphone_mixes` (add,
  change, remove, copy the main mix, and each track's level, pan and
  mute in a mix) and `set_monitor` (the speakers' controls, talkback,
  and where speakers, headphones and the talkback microphone are
  plugged in on this computer, so no undo step).
- **Files:** `save_project`, `open_project`, `new_project`, `export_mix`,
  `export_sections`, `export_as_project`, `export_stem_package`,
  `import_from_project`, `export_project_file`, `import_project_file`.

What each may do is held to its permission: reading, hearing and
seeing need Look only (`render_audio` writes its WAV to the app's cache
folder), changes need Edit, and saving and exporting need Edit and
export.

## The rule for new features

A feature is not done until an assistant can use it. In practice:

- Every action goes through the command table (`COMMANDS` in
  `crates/app/src/commands.rs`), so menus, keys, command search, the
  phone remote and assistants all call the same thing.
- A button, menu or control in a pane or window that changes the
  project runs a command, or does what an assistant tool also does.
  Each control says which, in a comment on its line or one of the three
  above it, and a test checks the name is a real command or tool:

  ```rust
  // ai: add-bus          (a command's id)
  // ai: set_parameter    (an assistant tool)
  // ai: view             (it only changes what is shown)
  ```

  Controls drawn before this rule are counted per file in
  `crates/app/widgets-unmarked.txt`; a file's count may only go down
  (`widget_check.rs`). The test prints the counts to copy in when
  marking old controls lowers them.
- The tests in `agent::coverage` run every command from an assistant
  with no one at the computer. A new command that can't run that way
  fails CI.
- A command that opens a window to ask for a file goes in
  `agent::NEEDS_A_PERSON`, and also in `agent_files::FILE_COMMANDS` or
  `agent::OWN_TOOL`, so an assistant can give the file instead. A test
  fails if it is in neither.
- A command whose dialog asks for settings (a name, a format, a range)
  needs a tool that takes those settings, as `save_project` and
  `export_mix` do.
- A new setting that can be changed by value shows up in
  `list_parameters` and `set_parameter`; a new part of the project
  shows up in `get_project`.
- Test a feature end to end through the same calls, with the test
  `Driver` in `agent::driver`.
- A new pane goes in `workspace::PANES`, and the test
  `a11y::every_pane_names_its_controls_and_says_its_values` then checks
  it: every control has a name in words (not only a symbol, so give an
  icon button `.accessible_name(...)`), and every slider says its value
  with its unit through `spoken::value`, `spoken::level` or
  `spoken::pan` (a bare 0.3 tells a screen reader nothing). Things drawn
  by hand that a person picks out (clips, notes, lanes) are focusable
  entries with a spoken name, as in the timeline, note editor and
  automation pane. Controls keep their places as state changes
  (`a11y::controls_stay_in_place`).
