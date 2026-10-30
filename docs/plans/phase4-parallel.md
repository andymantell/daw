# Phase 4: what to build next, and in what order (2026-10-30)

Phase 3 is merged. This plan takes the items left in
[ROADMAP.md](../ROADMAP.md) Phase 4 (collaboration depth and
interchange), adds the agent interface server from 2.11 (its command
layer is built, the server is not), pulls forward two items from later
phases that the gap survey ranked highest (audio that follows tempo from
Phase 6, and pitch editing from Phase 7), and adds the gaps marked
**High** in [research/FEATURE_GAPS.md](../research/FEATURE_GAPS.md).
Read on main at 6a4d9e7.

Priorities lean towards the owner's own use: playing guitar through the
app, and lessons with their teacher. Items that serve that come first in
each set.

Each item below is one feature thread with its own branch and PR. Items
in the same set don't share the files that carry their real work, so
they can run side by side. As before, `app/main.rs`, `app/commands.rs`
and `project/model.rs` are touched lightly by nearly every item; that is
handled by rebasing, and by each item keeping its work in its own module
with only a small hook in the shared files.

**Every item is done only when its commands and queries work from
outside the UI**, through the shared command table (2.11). Once A1 lands
its headless test driver, each item also adds at least one end-to-end
test that drives its feature through the agent interface.

## Set A: start now

| # | Item (plain English) | From | Main code areas | Needs from the owner |
|---|---|---|---|---|
| A1 | **Agent interface server**: the local server from 2.11, off by default, paired with a token, with per-session consent (read only; edit; edit and export) and a stop key. Perception: the project document, transport, selection, meters, and analysis on demand (spectrum, loudness, peak and RMS of a track or bus over a time range, the mix with one track removed), rendered audio of a region, and a picture of a pane. Action: every command in the table, any parameter by value, one labelled undo step per request. A headless mode for end-to-end tests. Land the headless test driver in an early, small PR so the other items can use it. | 2.11 | new `agent` crate, queries in `app/commands.rs`, an analysis module in `engine` | Connecting an assistant to it on the PC and trying a request. |
| A2 | **Low-latency monitoring with a big mix buffer**: live inputs, and the tracks being monitored through effects such as the amp, run on a small buffer, while the rest of the mix runs ahead on a large one. Delay compensation keeps the two in time, and the switch between them is automatic for armed and monitored tracks. | gaps (High) | `engine/graph.rs`, `engine/process.rs`, `audio-io` | Playing guitar through the amp on the PC at a small buffer with a heavy mix. |
| A3 | **Track and effect chain presets**: save a whole track (its effects, amp, sends and settings) or just its effect chain as a preset, then add it to any project. A preset library with tags, the factory presets kept apart from the user's, and presets that travel in bundles so a teacher can send a tone to a student. | gaps (High) | new `app/presets.rs`, `project` (preset format), `app/fx.rs` (save and load entries) | Which tones ship as factory presets. |
| A4 | **Song sections and insert or delete time**: an arranger track of named sections (verse, chorus, solo) that can be played in any order and repeated, then flattened into a plain timeline; practice looping by section; and inserting or deleting time across every track, tempo change, marker, automation and note. The insert and delete operation lands first in a small PR, because flattening uses it. | gaps (High) | new `app/sections.rs`, `project/model.rs` (sections), a time-shift operation in `project` | Trying a section loop in a lesson. |
| A5 | **Bring parts in from another project, and send parts out**: import chosen tracks, markers, tempo, sections or settings from another project, with their audio; export chosen tracks or a time range as a new project; and stem packages (audio and note stems as a folder or archive). A teacher can reuse lesson parts across students. | gaps (High), Phase 4 | new `app/import_project.rs`, `project/folder.rs`, `engine/export.rs` (called, not changed) | None. |
| A6 | **Note effects**: real-time effects on notes before they reach an instrument: arpeggiator, chord, note length, velocity, randomise, and scale (using the project's key and scale from 3.1). They sit at the head of an instrument track's chain and can be rendered to notes. | gaps (High) | new `engine/notefx.rs`, `app/fx.rs` (list entries) | Playing a keyboard through them. |
| A7 | **Audio that follows tempo**: audio clips can be set to musical time, so they follow tempo changes and the project tempo, stretched in real time and offline; stretch markers placed from detected attacks or by hand; dragging an attack onto a beat with no gaps or doubled attacks; audio quantise with strength and a window; and a tempo map made from a free performance. Built on our own stretcher behind its three calls, at risk on the 0.6 library choice. The interface for stretching a clip between markers lands first in a small PR, because B2 uses it. | Phase 6 | `engine/stretch.rs`, `engine/process.rs`, `app/timing.rs` (stretch markers), `app/tempo.rs` | Listening, and the 0.6 library choice. |
| A8 | **Interchange with other DAWs**: our own reader and writer for the open project-interchange format in the roadmap's Phase 4 (tracks, routing, clips with fades and loops, markers, notes with expression, automation, plugins with their state, the four generic built-in devices), with time conversion tests, an export report and an import report so nothing is dropped silently, and a fixture set of round-trip tests. | Phase 4 | new `interchange` crate, a File menu entry | Exporting a song from another DAW to try on import. |
| A9 | **Teacher with many students**: a lesson library, a project per student made from a lesson template, and a view of every student's assignments and practice log at a glance. | Phase 4 | `app/lessons.rs`, `app/start.rs`, `project/lessons.rs` | Trying it with the guitar teacher. |
| A10 | **Activity history and an undo list**: who changed what and when, read from the project's change history, and a list of undo steps to jump back several at once. | Phase 4, gaps | new `app/history.rs`, `project/doc.rs` (reading history) | None. |
| A11 | **Track versions**: alternative versions of a track's clips and settings, kept side by side and switched in one click, merging cleanly when shared. | Phase 4 | new `app/versions.rs`, `project/model.rs` (a version list per track) | None. |

A2 and A7 both touch `engine/process.rs`: A2 in how blocks are
scheduled, A7 in how a clip reads its audio. A3 and A6 both add entries
to `app/fx.rs` only. A4 and A11 both add to `project/model.rs`; each
keeps its own types. A5 and A9 both read other projects in the projects
folder; A9 calls A5's import once it lands and makes copies of templates
until then.

## Set B: start as the items they build on merge

| # | Item | From | Starts after | Main code areas |
|---|---|---|---|---|
| B1 | **Review mode**: proposals made on a branch of the project, shown as a diff with accept and reject per change, built on the snapshot diff and the activity history. A teacher can suggest edits to a student's take without overwriting it. | Phase 4 | A10 | new `app/review.rs`, `project/diff.rs` |
| B2 | **Pitch editing**: built-in note-by-note pitch editing of a single voice or a single-note guitar line: detected notes shown as blobs over the audio, moved in pitch and time, with drift, vibrato and formant control, rendered with the stretcher. | Phase 7 | A7's early PR | new `app/pitch_edit.rs`, a pitch detector in `engine` (from the tuner) |
| B3 | **Export a file per section**: a practice file for each section or marker range, with an export queue. | gaps | A4 | `app/export.rs` |
| B4 | **Switching container**: one container that switches between chains, such as clean and driven amp channels, from a key, a footswitch or automation. Pairs with A3, so a preset can hold both channels. | gaps | A3 | `engine/graph.rs` (a container kind), `app/fx.rs` |

## Set C: waiting on the owner, or later

- **Live lesson call** (Phase 4): audio and video between teacher and
  student in a session, music-quality audio, and the teacher's playback
  sent to the student. Waits on live sessions, which wait on the
  owner's network test (0.5).
- **Connectivity hardening** (Phase 4): better connection setup,
  diagnostics and fallbacks. Waits on the same test.
- **Small items**, for any thread with room: keeping recent notes played
  without Record on (3.1); chaining several commands into one key
  (macros); finding and removing audio no longer used; turning a track off
  to save processing; solo-safe and a listen bus; starting with every
  plugin off after a crash.

## From the user wish-list research

The survey is in [research/FEATURE_GAPS.md](../research/FEATURE_GAPS.md)
under "What users ask for". Most requests overlap items already here
(A1, A3, A4, A7, B2). Its three **High** wishes are suggested for the
owner to choose from, not yet started: a simpler mode for beginners,
guided audio setup, and accessibility across the whole app.

## Still waiting on the owner

Unchanged: live sessions and peer-to-peer transfer (network test), real
deletion out of history and keys per project (privacy decisions), signed
installers (0.7), the speed and pitch library choice (0.6 listening),
decision 0009 (notation formats), and our own sound content (3.4). New in
Phase 4: A7 and B2 are built at risk on 0.6, and which factory presets
ship (A3) is the owner's call. Each Phase 4 thread adds its own review
steps as a new file in [review-queue/](../review-queue/).
