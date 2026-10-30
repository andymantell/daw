# Architecture (proposed)

The target architecture for a native desktop DAW. It is designed so the
Phase 1 vertical slice grows into the final-phase workhorse without a
rewrite. It assumes **Rust** (decision 0003). If the Phase 0 spike sends us
to the C++/JUCE fallback, the structure below still applies, but the
libraries change. Items marked **(candidate)** are confirmed in Phase 0, and
their licences are verified by the CI licence check before adoption.

---

## 1. Overview

```
┌──────────────────────────────────── Application process ────────────────────────────────────┐
│                                                                                              │
│  UI thread                      Document layer                 Engine control (non-RT)       │
│  ┌──────────────────────┐ r/w  ┌───────────────────────┐ diffs ┌──────────────────────────┐  │
│  │ Timeline · Mixer ·   │◄────►│ Project document      │──────►│ Graph compiler           │  │
│  │ Looper · Editors ·   │      │ (CRDT, on disk)       │       │ (doc → render graph,     │  │
│  │ Plugin windows       │      └──────────┬────────────┘       │  latency compensation)   │  │
│  └──────────────────────┘                 │                    └────────────┬─────────────┘  │
│                                           │ sync                            │ atomic swap +  │
│                                           ▼                                 ▼ lock-free cmds │
│                              ┌────────────────────────┐        ┌──────────────────────────┐  │
│                              │ Collaboration service  │        │ Real-time audio thread(s)│  │
│                              │ (P2P sessions, bundle  │        │ DSP engine · scheduler · │  │
│                              │  import, folder watch) │        │ recorder · meters        │  │
│                              └──────────┬─────────────┘        └──────┬──────────┬────────┘  │
│                                         │                             │          │           │
│  Disk: project folder (document + content-addressed assets) ◄─────────┘          │           │
└─────────────────────────────────────────┼────────────────────────────────────────┼───────────┘
                                          │                                        │ shared memory
                                          ▼                                        ▼
                                 Other peers (teacher/student)       Plugin host process(es)
                                 direct P2P, no server we run        VST 3 · CLAP · AU (sandboxed)
                                                                     ▲
                                                Audio drivers: ASIO · WASAPI · CoreAudio · ALSA · JACK · PipeWire
```

Rules that hold everywhere:

1. **The project document is the single source of truth.** The UI and
   the engine are both projections of it.
2. **Audio threads never allocate, lock, do I/O, or wait** on the UI, the
   network or plugin scanning. They receive pre-built graphs and parameter
   changes through lock-free queues.
3. **Real-time and offline rendering share one code path.** Export, freeze,
   frozen-audio generation and tests all use the same engine.
4. **Assets are immutable and content-addressed.** Edits change clip
   metadata or create new assets. Audio is never mutated in place.
5. **Untrusted input is data, never code.** Bundles, peer messages and
   plugin state from peers are validated. Nothing received is executed.

## 2. Project on disk

```
My Lesson.project/              (folder; a tar of it is a bundle)
  project.doc                   CRDT document (binary) + schema version
  assets/ab/abcdef….flac        content-addressed audio (SHA-256)
  peaks/abcdef….peaks           multi-resolution waveform data
  frozen/<track-id>-<hash>.flac frozen audio for plugin tracks (Phase 1: kept with the
                                other audio, as content-addressed 24-bit WAV)
  local/                        per-machine data, never shared (device profiles, UI layout)
```

- **Lossless master copies** (FLAC or WAV) in `assets/`.
- **Snapshots** are names for versions of the document, kept in the
  document itself (its `snapshots` map), so they travel with it. Restoring
  one writes that version's project back as a new edit; history is never
  wound back. A snapshot needs the history up to its version, so history
  can't be trimmed past the oldest snapshot. Comparing two snapshots (or
  one with now) reads both versions and lists what differs, matched by id.
- **Templates** are project documents holding only a project's set-up
  (tracks, effects, sections, tempo, notes; no audio or comments), kept
  outside any project as `DAW/Templates/<name>.dawtemplate` in the user's
  profile. A project started from one gets new ids throughout.
- **Garbage collection** removes unreferenced assets after a retention
  period, never while any snapshot references them.
- **Crash safety:** recordings stream to a temporary file and are moved
  into `assets/` once complete. Interrupted takes are recovered on next
  launch.
- **Shared-folder mode:** when the folder sits in a cloud-synced
  directory, the app watches for external changes. Each computer writes
  its own copy of the document (`Devices/<device id>.daw`) that no other
  computer writes, so there are no sync-conflict copies. It looks at the
  other computers' copies every two seconds and merges them. Opening a
  project merges every copy in the folder.

## 3. Project document (candidate CRDT: `yrs`, `automerge` or Loro)

- A typed schema layer over the CRDT. Every structure has a `schemaVersion`
  with tested migrations.
- Stable unique IDs for all entities. Ordering uses CRDT lists or
  fractional indices.
- **Per-user undo**, scoped to the user's own changes.
- **Real deletion:** saved files and bundles blank out of the history
  the text of deleted comments and the earlier text of changed ones, so
  it can't be read back. Changes keep their ids, so a scrubbed copy
  still merges with one that wasn't; when the two disagree on a value
  both already have, the merge takes only the changes it lacks.
- **Ephemeral state** (presence, playheads, selections, follow mode) is
  sent over the session channel and not stored.
- **Time:** musical positions in integer ticks (**960 PPQ**), absolute
  positions in sample frames. Each clip declares its time base (decision
  0005).
- **Identity:** each install keeps a secret, from which each project
  gets its own key pair (so projects can't be linked by key). Changes
  are attributed by public key, and roles (owner/editor/commenter) are
  stored in the document and enforced when merging.
- **Notation-ready note data:** notes carry optional performance
  attributes alongside pitch, time and velocity: string and fret,
  techniques (bend amount and shape, slide, hammer-on/pull-off, harmonic,
  palm mute…), drum articulation and notehead, and tie/tuplet hints. Tab,
  staff and drum staff are views computed from this data plus the track's
  tuning or drum map, so no separate notation document can drift out of
  sync (Roadmap 3.8, decision 0009).
- **Interchange-ready from day one:** the DAWproject exporter arrives in
  Phase 4, but the schema must be able to express everything DAWproject
  1.0 can from the start: nested clips, warp points, mixed beat/second
  time units per clip, automation points with interpolation, sends,
  clip slots/scenes, per-note expressions, and opaque plugin state.
  Then the converter is a mapping layer, not a model change. Schema
  reviews check new structures against the DAWproject reference.

## 4. Audio engine

### 4.1 I/O (candidate: `cpal`)
- WASAPI and **ASIO** on Windows, CoreAudio on macOS, ALSA/JACK on Linux,
  with PipeWire through its JACK or ALSA compatibility.
- A device profile per machine: driver, device, sample rate, buffer size,
  measured round-trip and recording offsets.
- Handles device loss and hot-plugging without crashing the session:
  transport stops cleanly and resumes on reconnect.

### 4.2 Graph compiler (non-real-time thread)
- Turns the document into an immutable **render graph**: nodes, edges,
  buffers and per-node latency. **Delay compensation** is computed across
  the whole graph, including sends, sidechains and plugins.
- The new graph is swapped in atomically. Old graphs are freed off the
  audio thread.
- Parameter-only edits bypass recompilation and use the parameter queue.

### 4.3 Real-time processing
- Phase 1: a single real-time thread. Phase 8: multi-core processing,
  with a work-stealing scheduler over the graph's dependency levels.
- Lock-free single-producer/single-consumer queues for commands, meters
  and recorded input. A recorder thread moves captured audio from the
  ring buffer to disk.

### 4.4 Scheduler
- A sample-accurate transport. The tempo map converts ticks to samples.
- Clip playback, metronome, looper actions and Clip Grid launches are all
  **quantised events** in one scheduler.
- Loop boundaries are sample-accurate, with short crossfades.

### 4.5 Processor interface
- One internal trait for all devices (built-in and hosted): parameter
  descriptors, `process(block)`, latency, state save/load, bus layout.
- Built-in devices (EQ, amp sim, stretch…) and plugin adapters both
  implement it.

## 5. Plugin hosting

```
 Engine ──(shared-memory audio buffers + lock-free event queues)──► Plugin host process
                                                                     ├─ VST 3 adapter (candidate: `vst3` bindings)
                                                                     ├─ CLAP adapter  (candidate: `clack`)
                                                                     ├─ AU adapter    (macOS, own bindings)
                                                                     └─ ARA 2 adapter (Phase 7)
```

- **Scanning always runs out of process.** Results are cached, and plugins
  that crash or hang are blacklisted with a reason.
- **Hosting modes** (Phase 2): in-process for lowest overhead, a shared
  host process, or one process per plugin, chosen per plugin. Phase 1
  ran everything in-process; Phase 2's first cut (decision 0010) talks
  to the host process over pipes, one block behind, with shared-memory
  buffers as a later step if the pipe's cost shows.
- **Editor windows:** the plugin's view is embedded in a native child
  window we create (HWND / NSView / X11 window), with resizing,
  DPI scaling, focus and keyboard forwarding handled per OS.
- **State:** the plugin's opaque state is stored in the project as an
  asset, so it syncs to collaborators.
- **Missing plugins:** collaborators without a plugin hear its **frozen
  audio**, re-rendered automatically when that track changes, and its
  state is preserved untouched.
- **Apple Silicon:** native arm64 host. Intel-only plugins need a bridge
  process running under Rosetta, or are marked unsupported (decision in
  0.7).

## 6. Collaboration (no server we operate)

Three ways to share, all over the same CRDT document:

| Mode | How | Live? |
|---|---|---|
| **Bundle** | Export the document and the audio it uses as one uncompressed tar file of the project folder (exports left out). Import merges into an existing copy, open or among the user's projects, found by shared document history; otherwise it becomes a new project. Everything in a bundle is checked: plain asset names only, lengths within the file, audio matching its hash. | No |
| **Shared folder** | The project folder sits in a cloud-synced directory. Each computer writes its own copy of the document, and merges the others' as they arrive. | Near-live, at the provider's speed |
| **Live session** | Teacher hosts. Student joins by invite code, peer-to-peer. | Yes |

**Live session transport (candidates, chosen by spike 0.5):**
- WebRTC data channels in native code. NAT hole punching via STUN, using
  public STUN servers or configurable ones.
- QUIC with port mapping through UPnP/NAT-PMP/PCP on the host's router.
- In every option: the **invite code carries the host's connection
  details**, signed by the host's key. If no automatic exchange is
  possible, the two users swap codes by hand.
- Encrypted, mutually authenticated transport, keyed to project members'
  public keys.
- Known limitation: some networks (carrier-grade NAT on both sides, strict
  firewalls) block direct connections. Fallbacks are bundle or shared
  folder, and possibly a relay that users run themselves (open question,
  decision 0004).

**Asset transfer:** chunked, resumable, hash-verified, and prioritised by
what is needed for playback first.

**Live lesson call (Phase 4):** audio and video in the same peer-to-peer
session, with music-quality audio.

## 7. UI (toolkit decided in spike 0.1C — decision 0007)

- **Candidates:** Slint, egui, iced, Vizia, or a custom GPU-drawn UI on
  `wgpu` with a minimal widget layer. Possibly a hybrid: a standard
  toolkit for panels and dialogs, plus a custom GPU canvas for the
  timeline, waveforms, meters and note editors.
- **Requirements:** 60 fps with large projects; embedding native plugin
  windows; text editing and IME; menus; drag-and-drop; high-DPI; **screen
  reader support** (e.g. via AccessKit, to be verified per toolkit).
- Keyboard-first: command palette, fully remappable shortcuts.
- An original visual language, not imitating any existing product (see
  [CLEAN_ROOM.md](CLEAN_ROOM.md)).

## 8. Other libraries (candidates)

| Need | Candidates |
|---|---|
| MIDI I/O | `midir` |
| Audio file decode | `symphonia`, `claxon` (FLAC) |
| Audio file encode | `hound` (WAV), libFLAC bindings, Opus/MP3 encoders |
| Lock-free queues | `rtrb`, `ringbuf` |
| Real-time-safe memory reclamation | `basedrop`-style deferred drop, `arc-swap` |
| Stretch / pitch | Decided in spike 0.6 (licence-sensitive) |
| Neural amp model inference (Phase 2) | Neural Amp Modeler core library (reported MIT, to verify) or a pure-Rust port such as `nam-rs` |
| ML inference (Phase 7) | ONNX runtime bindings or a pure-Rust runtime |
| Port mapping (UPnP/NAT-PMP) | `igd-next` or similar |
| WebRTC / QUIC | `webrtc-rs`, libdatachannel bindings, `quinn` |

## 9. Distribution & updates

- **macOS:** signed with an Apple Developer ID and notarised. Universal
  or separate builds for Apple Silicon and Intel.
- **Windows:** code-signed installer (MSI or MSIX).
- **Linux:** AppImage and/or Flatpak (decided in 0.7). Real-time
  scheduling guidance for JACK/PipeWire users.
- **Updates:** check a static release feed (e.g. GitHub Releases), with
  signed update packages. No server we operate.
- **Crash reports:** written locally. The user chooses whether to send one
  (for example by attaching it to an issue).

## 10. Repository layout (proposed)

```
crates/
  app/              # the desktop application (UI, windows, menus)
  ui-kit/           # widgets, GPU canvas renderers (timeline, waveforms, meters)
  schema/           # project document types, migrations
  engine/           # graph compiler, scheduler, real-time processing
  audio-io/         # device abstraction over the I/O library
  effects/          # built-in effects (EQ, comp, reverb; amp sim to come)
  plugin-host/      # out-of-process plugin host binary + IPC
  plugin-formats/   # VST 3 / CLAP / AU / ARA adapters
  collab/           # sessions, bundles, folder sync, asset transfer
  notation/         # tab, staff and drum layout, MusicXML, text tab, fret assignment
  test-audio/       # golden-render fixtures and comparison tools
docs/
  ROADMAP.md · ARCHITECTURE.md · CLEAN_ROOM.md · GLOSSARY.md
  decisions/ · research/
spikes/             # Phase 0 throwaway prototypes (not shipped)
```

## 11. Testing strategy

- **Unit:** schema, migrations, tick/sample maths, DSP.
- **Golden renders:** fixture projects rendered offline, compared against
  references within tolerance (per-sample and spectral).
- **Null tests:** routing and delay compensation; equivalent paths must
  cancel to silence.
- **Real-time safety:** an allocation/locking detector on the audio thread
  in debug and CI builds.
- **Plugin host:** a suite of freely redistributable test plugins
  exercising latency, state, bus layouts, crashes and hangs.
- **Convergence:** randomised multi-peer edit sequences, including
  bundle and shared-folder merges, must converge.
- **Performance:** a CI benchmark of tracks/plugins processed within the
  real-time budget. Regressions past a threshold fail the build.
- **End-to-end:** UI automation of the Phase 1 user stories on all three
  operating systems.
