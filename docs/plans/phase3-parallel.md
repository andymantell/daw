# Phase 3: what to build next, and in what order (2026-10-30)

Read from [ROADMAP.md](../ROADMAP.md) Phase 3 (3.1 to 3.10) on main at
8ac3d86. Phase 2 is merged except automation (PR #43) and the first cut of
fixing a badly timed take, which are in progress.

Each item below is one feature thread with its own branch and PR. Items in
the same set don't share the files that carry their real work, so they can
run side by side. As in Phase 2, `app/main.rs` and `project/model.rs` are
touched lightly by nearly every item. That is handled by rebasing, and by
each item keeping its work in its own module with only a small hook in the
shared files.

Most of Phase 3 stands on notes: instruments, patterns, notation and
layered instruments all need the note data and instrument tracks from A1.
So A1 should land its note model and its note event type in an early,
small PR, before the rest of its work, so the Set B items can start sooner.

## Set A: start now

| # | Item (plain English) | Roadmap | Main code areas | Needs from the owner |
|---|---|---|---|---|
| A1 | **Notes and instrument tracks**: notes in the project (with room for optional string, fret, technique and drum articulation, as decision 0009 proposes), MIDI keyboards and controllers in and out on every operating system, instrument tracks, recording notes (overdub or merge, and keep what was just played), a first note editor (draw, move, velocity, quantise with strength and swing, transpose), and reading and writing standard MIDI files. The test plugin gains a simple instrument so the whole path can be tested in the cloud. | 3.1 | `project/model.rs` (notes), new `engine/midi.rs`, `audio-io` (MIDI devices), new `app/notes_editor.rs`, `test-plugin` | Playing a MIDI keyboard into it on the PC. |
| A2 | **The open plugin format**: hosting CLAP plugins (open SDK, MIT) beside the current format, for effects first, with scanning, the plugin's own window, presets, and the separate plugin process from Phase 2. Instruments in this format follow once A1 merges (B2). | 3.2 | `plugins` crate (a new `clap` module beside the current host) | Trying it with real plugins in that format on Windows. |
| A3 | **Built-in effects, set 2**: modulation effects (chorus, flanger, phaser, tremolo), multi-stage saturation, a transient shaper, a de-esser and multiband dynamics, in the effects list like the Phase 2 effects. | 3.5 | `effects` crate (new modules), one list entry each in `app/fx.rs` | Listening to each effect. |
| A4 | **Effect containers**: parallel chains with dry and wet, chains split by frequency band, mid/side and left/right split chains, holding built-in effects and plugins alike. Layered instruments come after A1 and B3. Delay compensation inside a container uses the Phase 2 work. | 3.5 | `engine/graph.rs` (the insert chain), `app/fx.rs` (chain view), `project/model.rs` (a container kind) | Trying it with real plugins. |
| A5 | **More export formats and stems**: export to FLAC, MP3 and Ogg/Opus, and export stems by bus. The encoders' licences are checked against GPLv3 first, and written up as a decision if any is new to the project. | 3.6 | `engine/export.rs`, `app/export.rs` | Listening to an exported file of each kind. |
| A6 | **Render in place and bounce**: freeze a track, render a track or group in place, bounce a selection, and paste a copied clip as rendered audio, all keeping the original so undo works. Built on the frozen-audio and export code. Automation is rendered once PR #43 merges. | 3.6 | `app/freeze.rs`, `engine/process.rs`, a few clip and track menu entries in `app/timeline.rs` | None. |
| A7 | **Start screen and faster loading**: a start screen with recent projects and templates, and projects that load their audio in parallel. | 3.6 | new `app/start.rs`, `app/project.rs` (loading) | Whether the start screen should show on every launch. |
| A8 | **Content browser**: browse the user's own sound folders and the project's audio, with tags, search and preview at the project's tempo (using the speed engine we have now, at risk on the 0.6 choice). | 3.6 | new `app/browser.rs`, a preview voice in `engine` | None. |
| A9 | **Lesson depth**: a practice log (time spent, loop counts and tempo reached per section), a snapshot per lesson with a timeline of lessons, and assignments ("bars 9–16 at 90 bpm by next week") whose progress both teacher and student see in the shared project. Works through bundles and shared folders, so it doesn't wait on live sessions. | 3.7 | `app/practice.rs`, `app/trainer.rs`, `app/notes.rs`, `project/model.rs` (log and assignments) | Trying it with the guitar teacher. |
| A10 | **Notation engine**: a new crate that lays out and draws tab (any string count, tuning and capo, with the techniques in 3.8), standard staff and a drum staff with a drum map from the standard percussion key map, using a bundled open-licence music font in the standard music-font layout. It also reads and writes MusicXML 4.0, writes plain-text tab and print/PDF, and assigns strings and frets to notes that come without them. It has its own score types and tests, so it doesn't wait on A1. Built at risk on decision 0009, which is still proposed. | 3.8 | new `notation` crate, a font in `app/fonts` with its licence | Accepting decision 0009. Reading the first engraved pages. |
| A11 | **Video file clips**: a video clip on the timeline with its picture in a pane, its sound as an ordinary audio clip (so speed, transpose and looping apply), decoded with each operating system's own media framework so we ship no codec code. Cloud tests run on Linux; Windows decoding is built at risk. | 3.9 | new `video` crate, new `app/video.rs`, a video clip kind in `project/model.rs` | Trying video files on the PC. |
| A12 | **Timecode and hit points**: a timecode ruler and position display beside bars and beats, at the frame rates in 3.10 including drop-frame counting, a start timecode and offset, hit-point markers, and a tempo fit tool that lands chosen hit points on beats or bars. Needs no video decoding, so it runs beside A11. | 3.10 | `engine/time.rs` (timecode), the ruler in `app/timeline.rs`, new `app/hitpoints.rs` | None. |

A1 and A10 both handle notes. A1 owns the project's notes; A10 keeps its own
score types, and B5 joins the two. A3 and A4 both touch `app/fx.rs`; A3 only
adds list entries. A6 and A12 touch `app/timeline.rs` lightly (menus and the
ruler). A5 and A6 share export code; A6 calls into it and doesn't change it.

## Set B: start as the items they build on merge

| # | Item | Roadmap | Starts after | Main code areas |
|---|---|---|---|---|
| B1 | **Note editing depth**: humanise, legato, scale mode (a key and scale for the project and each clip), selecting and filtering notes by time, pitch, velocity, probability or length, controller lanes, per-note expression curves, MIDI learn, and groove templates taken from audio or notes. | 3.1 | A1 | `app/notes_editor.rs`, new `engine/groove.rs` |
| B2 | **Plugin instruments**: instruments in both plugin formats, including instruments with several outputs, each output on its own track (the item left over from Phase 2 plugin depth). | 3.2 | A1, A2 | `plugins` crate, instrument tracks in `engine/graph.rs` |
| B3 | **Built-in instruments**: a multi-zone sampler, a drum sampler, a synth of our own design, a drum machine (layered pads mixing samples and synthesised drum voices, choke groups, an effect chain, outputs and sends per pad), and a slicer that turns a loop into a playable instrument. | 3.4 | A1 | new `instruments` crate, `app/fx.rs` (list entries) |
| B4 | **Step sequencer and patterns**: a drum pattern editor with swing, probability, Euclidean fills, per-step velocity and pitch, per-lane length, resolution and direction, repeats, micro-offset and gate; patterns as clips, linked clips that share one pattern, and chance across groups of notes. Drives the drum machine from B3. | 3.3 | A1 (B3 for the drum machine link) | new `app/patterns.rs`, `project/model.rs` (patterns) |
| B5 | **Notation lanes on the timeline**: A10's tab and staves shown under an instrument track as a view of its notes, scrolling with playback, following tempo, bars and loop; tab entry by keyboard and from the note editor; MusicXML and MIDI import that makes tracks; practice tie-ins (practice-mute the part, count-in following the notation's bars). | 3.8 | A1, A10 | new `app/notation.rs`, `notation` crate |
| B6 | **Layered instruments**: several instruments sharing one set of notes, in a container. | 3.5 | A1, A4 | `engine/graph.rs`, `app/fx.rs` |
| B7 | **Scoring to picture**: the filmstrip of frame thumbnails on the video clip, built on import and cached; a frame-accurate video pane with a timecode overlay and frame stepping; hit points shown in the video pane; and export of a copy of the video with the mix, without re-encoding the picture. | 3.10 | A11, A12 | `video` crate, `app/video.rs` |

## Set C: later

- **The macOS plugin format** (3.2): hosting it, after B2. CI already builds
  on macOS, but there is no Mac to try it on, so it would be built and
  checked by compiling only.
- **Online video links as reference clips** (3.9): the platform's own
  player in an embedded web view, kept in sync with the transport. This
  starts with a spike on adding a web view to the app and on how well an
  embedded player stays in sync. Adding a web view is a sizeable new
  dependency, so the spike's result goes to the owner before the feature
  is built. After A11.
- **Tab entry from a MIDI guitar controller** (3.8), after B5. Entry from
  recorded audio waits on audio-to-MIDI in Phase 7.

## Still waiting on the owner

Unchanged: live sessions and peer-to-peer transfer (network test), real
deletion out of history and keys per project (privacy decisions), signed
installers (0.7), and the speed and pitch library choice (0.6 listening).
New in Phase 3: our own sound content (3.4) needs recordings made or
licensed, which is the owner's call; the synthesised drum voices in B3 don't
wait on it. Each Phase 3 thread adds its own review steps as a new file in
[review-queue/](../review-queue/).
