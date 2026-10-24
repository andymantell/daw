# Architecture (proposed)

The target architecture, designed so the Phase 1 vertical slice grows into
the Phase 9 workhorse DAW without a rewrite. Items marked **(proposed)** are
confirmed or replaced by the Phase 0 spikes. Results are recorded in
[`docs/decisions/`](decisions/).

---

## 1. Overview

```
┌──────────────────────────── Browser tab (or Electron renderer) ───────────────────────────┐
│                                                                                            │
│  UI (main thread)                    Document layer                 Engine host            │
│  ┌────────────────────┐   reads/   ┌──────────────────────┐ diffs  ┌─────────────────────┐ │
│  │ Timeline / Mixer / │◄──writes──►│ Project document     │───────►│ Graph compiler      │ │
│  │ Looper / Editors   │            │ (CRDT, local-first)  │        │ (doc → render graph)│ │
│  └────────────────────┘            └─────────┬────────────┘        └──────────┬──────────┘ │
│                                              │ sync                           │ commands   │
│                                              ▼                                ▼ (lock-free │
│                                    ┌──────────────────────┐        ┌─────────────────────┐ │
│                                    │ Sync provider        │        │ Audio thread        │ │
│                                    │ (WebSocket, offline  │        │ AudioWorklet + WASM │ │
│                                    │  queue, IndexedDB)   │        │ DSP engine          │ │
│                                    └─────────┬────────────┘        └──────────┬──────────┘ │
│                                              │                                │            │
│  Asset store: OPFS cache ◄────────────────── │ ─────────────────────────────► │ audio I/O  │
└──────────────────────────────────────────────┼────────────────────────────────┼────────────┘
                                               ▼                                ▼
                                   Sync server + object storage       Web Audio output / input
                                   (rooms, auth, assets)              (or native backend in desktop)
```

Rules that hold everywhere:

1. **The project document is the single source of truth.** UI and engine are
   both projections of it.
2. **The audio thread never allocates, locks or waits on the network.** It
   receives a pre-compiled graph and parameter changes through lock-free
   queues.
3. **Real-time and offline rendering share one code path.** Export, freeze
   and tests use the same engine, driven faster than real time.
4. **Assets are immutable and content-addressed.** Edits create new assets
   or change clip metadata. They never mutate audio in place.

## 2. Project document (proposed: Yjs CRDT)

- One CRDT document per project. It holds tracks, clips, devices and parameters,
  automation, tempo map, markers, comments, looper state and settings.
- **Schema-first:** a typed schema layer (TypeScript) wraps raw CRDT types,
  and every structure has a `schemaVersion` with tested migrations.
- **Identity:** every entity has a stable unique ID. Ordering uses CRDT
  sequences or fractional indices.
- **Undo:** per-user undo, scoped to that user's own changes.
- **Large data stays out:** audio lives in assets. Dense note data can live in
  compact binary sub-documents if profiling demands it.
- **Ephemeral state** (presence, playheads, selections, follow mode) uses the
  awareness channel, not the persisted document.
- **Time representation:**
  - Musical positions are integer ticks (proposed **960 PPQ**). Absolute
    positions are sample frames at the project sample rate.
  - Each clip declares a time base: musical or absolute. The tempo map
    converts between them.
- **Snapshots** are stored CRDT state vectors with a name and author, and can
  be restored as a new head.

## 3. Assets

- A recorded take is written to OPFS while recording, then hashed (SHA-256)
  and uploaded in the background with resumable chunks.
- The document references assets by hash, plus metadata: sample rate,
  channels, length, and peak data used to draw waveforms.
- Storage formats: lossless (FLAC or WAV) as the master copy. A compressed
  proxy (Opus) can be fetched first for quick previews on slow links.
- Garbage collection removes assets that no live document or snapshot
  references, after a retention period.

## 4. Audio engine

### 4.1 Runtime
- **AudioWorklet** hosting a **WASM DSP core** (proposed: **Rust**, see
  decision 0003). The core is plain portable code with no browser
  dependencies, so the desktop wrapper can compile the same crate natively.
- Main ↔ audio-thread communication uses **SharedArrayBuffer ring buffers**
  (commands, parameter changes, meters, recorded input). This requires
  **cross-origin isolation** (COOP/COEP headers). If isolation is
  unavailable, the engine falls back to `postMessage`.
- Phase 9 multi-core: split the graph across several worklets or WASM threads,
  scheduled by dependency.

### 4.2 Graph compiler (main thread or worker)
- Turns the project document into an immutable **render graph**: nodes,
  connections, buffers and latency per node. It is computed off the audio
  thread and swapped in atomically.
- Computes **processing-delay compensation** across the whole graph,
  including sends and sidechains.
- Edits that only change parameters skip recompilation and go through the
  parameter queue.

### 4.3 Scheduling
- A sample-accurate transport inside the engine. The tempo map converts ticks
  to samples.
- Clip playback, metronome, looper actions and Clip Grid launches are all
  **quantised events** in one scheduler (Phase 5 and 6 depend on this).
- Loop regions and looper loops wrap with sample accuracy and crossfades
  at the boundaries.

### 4.4 Processor API
- One internal interface for every device (gain, EQ, amp sim, stretch,
  instruments): a parameter descriptor list, `process(block)`, reported
  latency, and save/load of state.
- Built-in devices use the same API that hosted plugins later go through
  (WAM 2.0 adapter in Phase 7; native VST 3 / CLAP adapter in Phase 8).

### 4.5 Recording path
- Input is captured with `getUserMedia`, with `echoCancellation`,
  `noiseSuppression` and `autoGainControl` turned off, routed into the
  worklet, and written to the ring buffer and then to OPFS.
- Each take's timeline position is corrected by the calibrated round-trip
  offset for that device. The raw offset is stored with the take, so it can
  be re-corrected later.

## 5. Audio I/O abstraction (web ↔ desktop)

```
            ┌────────────── Engine interface ───────────────┐
            │ start/stop · devices · process callback ·     │
            │ latency report · input streams                │
            └──────────────┬──────────────────────┬─────────┘
                           │                      │
                Web backend (default)      Native backend (Electron, optional)
                AudioWorklet + WASM        Native module: same Rust DSP core,
                Web Audio I/O              ASIO / CoreAudio / JACK / PipeWire
```

- The UI and document layer never know which backend is active.
- In desktop builds, the DSP core runs natively in a separate process. The
  renderer talks to it over shared memory or IPC, using the same command
  protocol as the worklet.
- Native plugins (Phase 8) run in a plugin host process. A project that
  uses them carries frozen audio, so web-only collaborators can still play it.

## 6. Collaboration & backend

- **Sync server** (proposed: y-websocket-compatible). It handles room
  membership, auth checks per document, and persistence of document updates
  to a database. It scales horizontally by room.
- **Auth:** email magic link and OAuth. Role-based access per Room (owner,
  editor, commenter, viewer).
- **Object storage:** S3-compatible, reached through signed upload and
  download URLs.
- **Live call (Phase 4):** WebRTC with self-hosted STUN/TURN. Music mode
  turns voice DSP off and uses a higher Opus bitrate.
- **Offline:** the full document lives in IndexedDB. Pending uploads are
  queued, and everything merges on reconnect.
- **Privacy:** data minimisation, encryption in transit and at rest,
  account and data deletion, and child-data compliance review for lesson
  features.

## 7. UI

- **Framework: open decision for Phase 0.** Candidates are React or Svelte
  with TypeScript. The choice matters less than the rendering approach
  below.
- **Timeline, waveforms, meters and note editors** draw on **Canvas/WebGL**
  with virtualised rendering, not DOM nodes per element, so large projects
  stay at 60 fps.
- Peak data for waveforms is precomputed at several zoom levels and stored
  with each asset.
- Keyboard-first, with a command palette and fully remappable shortcuts.
- An original visual language. It does not imitate any existing product
  (see [CLEAN_ROOM.md](CLEAN_ROOM.md)).

## 8. Repository layout (proposed)

```
apps/
  web/            # the browser app (UI)
  desktop/        # Electron shell (Phase 8)
  server/         # sync server, auth, asset API
packages/
  schema/         # project document types, migrations
  engine-core/    # Rust DSP core (→ WASM and native)
  engine-web/     # AudioWorklet host, ring buffers, graph compiler
  devices/        # built-in processors (EQ, comp, amp sim, ...)
  ui-kit/         # shared UI components, canvas renderers
  test-audio/     # golden-render fixtures and comparison tools
docs/
  ROADMAP.md · ARCHITECTURE.md · CLEAN_ROOM.md · GLOSSARY.md
  decisions/      # decision records
  research/       # capability landscape (generic)
```

## 9. Testing strategy

- **Unit:** schema, migrations, tick/sample conversion, DSP functions
  (Rust tests).
- **Golden renders:** fixture projects rendered offline and compared with
  references using tolerance-based comparison (per-sample error and
  spectral difference).
- **Null tests:** routing and delay compensation, where the difference of
  two equivalent paths must be silent.
- **Convergence:** randomised multi-client edit sequences must converge to
  identical documents.
- **End-to-end:** Playwright tests of the user stories, using fake media
  devices that feed known audio.
- **Performance:** CI benchmark measuring how many tracks render before the
  real-time budget is exceeded. A regression past threshold fails CI.
