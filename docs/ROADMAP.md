# Roadmap

**Vision:** a web-first, collaborative digital audio workstation. It starts as
the best tool for remote and in-person **guitar lessons** and grows, phase by
phase, into a **full professional DAW**. It also has a visible, timeline-aware
**Live Looper** performance mode.

Guiding principles:

1. **Vertical slices, not layers.** Every phase ends with something a real
   person can use end to end. The first users are a guitar teacher and a
   student.
2. **Architecture for the destination, scope for the slice.** Phase 1 is
   small, but its data model, engine and sync layer must not need replacing
   to reach Phase 9.
3. **Web first, desktop optional.** Everything runs in a modern browser. An
   optional Electron wrapper (Phase 8) adds low-latency native audio and
   native plugins without forking the app.
4. **Collaboration is core, not an add-on.** The project document is
   shared, local-first and conflict-free from day one.
5. **Clean-room, original work.** See [CLEAN_ROOM.md](CLEAN_ROOM.md). Every
   feature is specified in generic terms and designed independently.
6. **Audio correctness is non-negotiable.** No dropouts, sample-accurate
   timing, deterministic offline renders, and automated audio tests.

How this document works:

- Phases are **sequential milestones**, and later phases can start in
  parallel once their dependencies are met (see the dependency map at the
  end).
- Each phase lists **goals, deliverables, exit criteria and risks**. A phase
  is done when its exit criteria are met, not when time runs out.
- There are deliberately **no calendar estimates**. Sizes (S/M/L/XL) are
  relative effort only.
- Feature coverage is cross-referenced in
  [research/CAPABILITY_LANDSCAPE.md](research/CAPABILITY_LANDSCAPE.md).

---

## Phase 0 — Foundations & technical spikes

**Goal:** answer the risky technical questions before building features. Set
up the repository so that quality is automatic.

### 0.1 Repository & tooling (S)
- Monorepo layout (see [ARCHITECTURE.md](ARCHITECTURE.md) §8).
- TypeScript strict mode, linting and formatting, and unit test runner.
- CI: lint, typecheck, unit tests, licence check of dependencies, build.
- Contribution rules: DCO sign-off, code of conduct, clean-room policy link.
- Decision records in `docs/decisions/` (template + first records).

### 0.2 Decide the project licence (S, *blocking for 0.6*)
- See [decisions/0001-project-licence.md](decisions/0001-project-licence.md).
  This affects which time-stretch libraries we can use and how the
  desktop ASIO build works.

### 0.3 Spike: audio engine in the browser (M)
- An AudioWorklet running a WASM DSP module, with a lock-free ring buffer
  (SharedArrayBuffer) between the main thread and the audio thread.
- Confirm the cross-origin isolation headers (COOP/COEP) work with our
  hosting, storage and auth providers. SharedArrayBuffer needs them.
- Measure CPU headroom: N tracks of playback + simple effects before
  dropouts, on a low-end laptop, on Chromium, Firefox and Safari.

### 0.4 Spike: latency measurement & compensation (M)
- Loopback calibration (play a click, record it back, measure the offset).
- Compare with `AudioContext.outputLatency` / `baseLatency` per browser.
- Record a guitar DI against a click and confirm the take is within
  ±1 ms of the grid after compensation, on at least: Windows/Chromium,
  macOS/Safari, macOS/Chromium, Windows/Firefox.
- Document the recommended monitoring setup (direct monitoring on the
  interface).

### 0.5 Spike: collaborative document (M)
- Prototype the project schema on a CRDT (Yjs proposed, see
  [decisions/0004-collaboration-model.md](decisions/0004-collaboration-model.md)).
- Two browsers editing tracks and clips at the same time. Offline edits merge
  on reconnect. Per-user undo.
- Asset upload flow: a recorded take goes to object storage, and the
  document references it by content hash.

### 0.6 Spike: pitch-preserving speed change (M)
- Evaluate stretch approaches for real-time playback at 50–150% speed and
  ±12 semitones. Licence-compatible open implementations vs. our own
  phase-vocoder/WSOLA-style implementation from the published literature.
- Listening tests with guitar material. Measure CPU per track.

### 0.7 Browser support matrix (S)
- Record the minimum browser versions and which features degrade where (e.g.
  no Web MIDI on Safari, `setSinkId` availability, multichannel input).

**Exit criteria**
- All spikes have a written result in `docs/decisions/`, with a go or
  no-go for the proposed approach.
- CI is green on an empty app skeleton, and the licence check is enforced.

**Key risks:** cross-origin isolation clashing with third-party
auth/embeds; Safari audio quirks; stretch quality vs CPU.

---

## Phase 1 — Vertical slice: **Lesson Room**

**Goal:** a teacher and a student can use the app in a real guitar lesson
and between lessons. Record ideas, practise against backing tracks at
reduced speed, and leave each other time-pinned feedback, in a shared
project that stays in sync.

**Primary user stories**
- *Teacher* creates a Lesson Room, imports a backing track, records an
  example riff on a new track, marks sections ("Intro", "Bars 9–16"), and
  shares a link.
- *Student* opens the link, loops "Bars 9–16" at 70% speed with the
  teacher's riff muted, records three attempts and leaves a question pinned
  at bar 12.
- *Teacher* listens to the attempts later, pins feedback on the second take,
  and records a corrected version.
- Both can tune up, use the metronome and work offline. Changes sync when
  they reconnect.

### 1.1 Engine core (L)
- Transport: play, stop, position, loop region, tempo, time signature
  (single tempo in this phase).
- Sample-accurate scheduler running in the worklet. Musical ↔ absolute
  time conversion.
- Audio clip playback from decoded assets, with streaming/caching for long
  files.
- Per-track gain, pan, mute, solo. Master bus. Peak meters.
- Metronome: accents, subdivisions, count-in, separate output level.
- Offline render path, sharing the same code as real time (it powers export
  and tests).

### 1.2 Recording (L)
- Input device and channel selection. Browser voice processing disabled.
- Arm, record, count-in, recording into a new take.
- Latency compensation from calibration (wizard from spike 0.4). Per-device
  calibration stored locally.
- Guidance for direct monitoring. Optional software monitoring with a clear
  latency warning.
- Takes saved locally first (OPFS/IndexedDB), then uploaded in the
  background.
- Multiple takes on one track: the newest is shown on top, older ones stay
  available (a simple version of take lanes, extended in Phase 2).

### 1.3 Practice tools (M)
- Loop region with quick selection by section/marker.
- Pitch-preserving speed control (50–150%) and transpose (±12 st).
- Tempo trainer: speed rises by X% every N loops up to a target.
- Chromatic tuner (pitch detection on the input). Reference pitch setting.
- Per-track "practice mute": mute the part being learnt.

### 1.4 Editing — minimal set (M)
- Select, move, trim, split, delete clips. Snap to beat/bar.
- Import audio files (drag and drop).
- Markers and named sections.
- Collaborative undo/redo (per user).

### 1.5 Collaboration (L)
- Accounts (email magic link or OAuth). Lesson Room with roles
  (teacher = owner, student = editor).
- Share by invite link. Revocation.
- Real-time sync of the project document. Presence (avatars, playheads).
- **Follow mode:** the student's view and playhead follow the teacher's
  during a live lesson.
- **Pinned comments:** comments on a time position/range on a track or the
  timeline, with threads, resolve/unresolve, and jump-to-play.
- Asset sync with progress, resumable uploads, and content-hash
  de-duplication.
- Local-first: the full project works offline and merges on reconnect.

### 1.6 Lesson notes & tab (S)
- Rich-text lesson notes per project.
- Plain-text tab blocks (monospace) that can be pinned to sections.

### 1.7 Output & safety (M)
- Export mixdown as WAV (and per-track stems).
- Snapshot on demand ("Lesson 3 end"). Restore a snapshot.
- Autosave, crash recovery.
- Privacy: data minimisation, account deletion, data export. Lesson users
  may include **children**, so design for applicable child-data rules (e.g.
  UK Age Appropriate Design Code / GDPR, COPPA in the US) before public
  launch.

### 1.8 Quality bar (M, runs throughout)
- No audio dropouts with 16 stereo tracks on the reference low-end laptop.
- Golden-file audio tests: offline renders of fixture projects compared
  against reference renders within a tolerance.
- End-to-end tests of the user stories above in at least Chromium and
  Firefox (Safari manually until automated).
- Accessibility: full keyboard operation of transport/recording, screen
  reader labels, colour-contrast compliance.

**Exit criteria**
- A real teacher and a real student complete **three consecutive lessons**
  and the practice in between using only this app, with no blocking bugs.
- Recorded takes land within ±1 ms of the grid on the supported matrix
  (after calibration).
- Offline edits from both users merge without data loss in the scripted
  conflict tests.

**Out of scope for Phase 1:** MIDI, effects beyond gain/pan, automation,
buses, tempo changes, comping UI, live video call.

---

## Phase 2 — Core audio DAW

**Goal:** a capable audio-recording and mixing DAW. Someone could record and
mix a full song of guitar, bass, vocals and imported drums.

### 2.1 Editing depth (L)
- Fades and crossfades (curve shapes), clip gain and envelopes.
- Nudge, slip-edit (move audio inside a clip), ripple editing, range tools.
- Grid settings, snap modes, zoom, multiple selection, keyboard-first
  editing.
- Customisable key commands with a searchable command palette.
- Destructive operations produce new assets (normalise, reverse, gain).

### 2.2 Recording depth (M)
- Punch in/out (manual and range), pre- and post-roll.
- Loop recording into stacked **take lanes**.
- **Comping:** swipe to choose the best parts, auto crossfades at edit points,
  audition takes, flatten a comp.
- Multi-input recording (multiple tracks armed), where the browser
  supports it.

### 2.3 Mixer & routing (L)
- Insert chains: add, reorder, bypass, wet/dry.
- Sends (pre/post fader), effect return buses, group buses, folder tracks.
- Control-group faders (VCA-style).
- Sidechain inputs.
- **Engine-wide processing-delay compensation**, including sends and
  sidechains.
- Mixer view inside the main window and as a separate full view.
- Metering: peak/RMS, LUFS integrated/short-term, true peak.

### 2.4 Built-in effects, set 1 (L)
- Parametric EQ with analyser. Compressor. Gate/expander. Limiter.
- Algorithmic reverb and convolution reverb (our own impulse responses).
- Delay (tempo-synced, ping-pong, filtered).
- **Guitar amp and cabinet simulation** (waveshaping + cabinet IRs we
  record or design ourselves). Tuner as an insert.
- All built on our internal processor API (see Architecture §4) so they
  later work as plugins too.

### 2.5 Automation (M)
- Draw and edit automation lanes for any parameter.
- Record modes: read, touch, latch, write.
- Clip-attached automation vs track automation.

### 2.6 Tempo & time (M)
- Tempo map with changes and ramps. Time-signature changes.
- Clips follow tempo changes (MIDI-free phase: audio stays in absolute time
  unless marked musical, ahead of Phase 6 stretching).

### 2.7 Project management (S)
- Named snapshots/versions with a diff summary ("3 tracks changed").
- Project templates (e.g. "Guitar lesson", "Band demo").

**Exit criteria**
- A full multitrack song (≥24 tracks, ≥10 effects) can be recorded, comped,
  mixed with automation, and exported, all in the browser, with no dropouts
  on the reference machine.
- Delay compensation is verified by automated null tests.

---

## Phase 3 — MIDI, instruments & practice depth

**Goal:** compose and arrange with virtual instruments, and broaden the
lesson toolkit.

### 3.1 MIDI foundation (L)
- Web MIDI input (Chromium, Firefox). Clear messaging where it is
  unavailable (Safari).
- Instrument tracks, MIDI recording, overdub/merge, retrospective capture
  (keep what you just played).
- Note editor: velocity and CC lanes, quantise (with strength/swing),
  humanise, legato, transpose, scale snapping.
- **Scale mode:** project/clip key and scale that guide editing.
- Standard MIDI file import/export.
- MIDI learn for controller mapping.

### 3.2 Pattern & step programming (M)
- Step sequencer / drum pattern editor: swing, per-step probability,
  Euclidean fill, per-step velocity/pitch.
- Patterns as reusable clips on the timeline.

### 3.3 Instruments (L)
- Multi-zone sampler (round robin, velocity layers) and drum sampler.
- Subtractive/wavetable synth (our own design and presets).
- Our own sound content: kits, presets and samples recorded or designed
  in-house or licensed for redistribution.

### 3.4 Effects, set 2 (M)
- Modulation (chorus, flanger, phaser, tremolo, auto-pan), saturation/
  distortion (multi-stage, multiband), transient shaper, de-esser,
  multiband compressor.

### 3.5 Workflow (M)
- Freeze and render in place. Bounce selection.
- Content browser: tags, search, preview at project tempo, favourites.
- Export formats: FLAC, MP3, Opus. Stem export by bus.
- Channel strip presets. Mixer snapshots.

### 3.6 Lesson depth (M)
- Practice log: time spent, loop counts, tempo reached per section.
- Lesson history: a snapshot per lesson, plus a timeline of lessons.
- **Tab view** of MIDI/notes (guitar/bass tunings), and tab entry that
  generates notes.
- Teacher assignment: "practise bars 9–16 to 90 bpm by next week" with
  progress visible to both.

**Exit criteria**
- A song can be composed using only built-in instruments, MIDI and audio.
- A teacher can set an assignment, and see the student's practice progress
  and tab-based exercises.

---

## Phase 4 — Collaboration depth & interchange

**Goal:** make co-working richer than in any single-user DAW.

- **Live lesson call** (L): WebRTC audio/video in the Room. Music mode
  (voice DSP off, high-bitrate Opus, stereo). Teacher can stream the
  project's playback to the student. Self-hosted TURN.
- **Review mode** (M): a collaborator proposes changes on a branch of the
  project. The owner sees a diff and accepts or rejects.
- **Track versions** (M): alternative versions of a track's content and
  settings, switchable and comparable.
- **Interchange** (M): import/export the open DAWproject format. Audio and
  MIDI stem packages.
- **Organisation features** (M): teacher with many students, rooms list,
  per-student defaults, bulk sharing.
- **Permissions & audit** (S): who changed what, when (activity feed).

**Exit criteria**
- A full lesson runs inside the app with video and shared playback.
- A project round-trips through DAWproject export and import without
  losing tracks, clips, tempo or automation that the format supports.

---

## Phase 5 — **Live Looper** mode

**Goal:** a live-looping performance mode with the immediacy of a
hardware loop station. Unlike hardware, every loop, layer, effect and
setting is **visible**: on a looper view and on the project timeline.

### 5.1 Looper engine (L)
- Any number of **loop tracks** (default 5), stereo, each with level/pan in
  the mixer.
- One-button cycle per track: record → overdub → play → stop. The order can
  be set per track (rec→dub→play or rec→play→dub).
- **Loop sync:** the first loop sets the master length, and others are
  multiples or divisions of it. Alternatively free-length tracks.
- **Quantised actions:** start/stop/record snap to beat, bar or loop end.
  Immediate mode available.
- **Tempo sync:** loops follow project tempo, with optional pitch-follows-
  tempo (uses the Phase 1 stretch engine).
- Per-track: one-shot vs loop, reverse, start mode (immediate/fade-in),
  stop mode (immediate/fade-out/at loop end), loop length in bars.
- Overdub modes: **layer**, **replace** and **punch-replace**.
- **Per-track layer history:** undo/redo the last layer, **bookmark** a
  layer and jump back to it.
- **Play mode:** *multi* (all tracks independent) or *single* (starting one
  stops the other, for verse/chorus switching).
- **Auto-record:** recording starts when input passes a threshold.
- **Bounce:** record the mix of playing loops into another loop track.
- Recording limited only by storage. Long loops stream to OPFS.

### 5.2 Effects for performance (L)
- **Input effects** (before recording) and **track effects** (on playback),
  several at once per section.
- **Effect banks:** recallable sets of effects, switchable live.
- Our own performance processors: filters (LP/BP/HP with sweep), beat
  repeat, roll, freeze, stutter/gate patterns, pitch bend, harmoniser,
  vocoder, lo-fi, radio, robot, tape stop, reverse-delay.
- Step-sequenced parameter changes on effects.

### 5.3 Rhythm (M)
- Built-in drum patterns and kits (our own content), with tempo, swing and
  fills. Or use any instrument/pattern track as the rhythm source.

### 5.4 Control (M)
- Keyboard mapping, Web MIDI controllers and footswitches. Press / hold /
  double-press can trigger different actions.
- A large-target touch layout for tablets.
- Assignable "macro" controls across multiple parameters.

### 5.5 Visible in DAW context (M) — *the differentiator*
- Looper view: each loop track shows its waveform, layer stack, position
  in the loop, state and active effects.
- **Every take and layer is a real asset** on the project timeline. A
  performance can be **captured to the timeline** as clips for later
  editing.
- **Phrase sets:** save and recall loop-track sets with their settings.
- Loops are collaborative objects: a teacher can prepare a phrase set, and
  a student performs over it.

**Exit criteria**
- A five-loop performance with input and track effects runs for 10
  minutes on the reference machine with no dropouts. Captured to timeline,
  it plays back identically (offline-render comparison).
- Loop alignment is sample-accurate after latency compensation (automated
  test).
- Performance-quality monitoring needs direct/hardware monitoring in the
  browser. The desktop wrapper (Phase 8) provides low-latency software
  monitoring through effects.

---

## Phase 6 — Clip Grid & musical time for audio

**Goal:** non-linear arranging and audio that follows tempo.

- **Clip Grid** (L): clip slots per track, quantised launch, scenes,
  launch modes (trigger, gate, toggle, repeat), follow actions,
  legato switching. Capture to timeline. Shares the scheduler with Live
  Looper.
- **Audio follows tempo** (XL): high-quality offline and real-time stretch,
  multiple algorithms (rhythmic, tonal, complex), with a quality/CPU trade-off.
- **Stretch markers** (M): manual and automatic (from transients) anchors.
- **Transient detection and slicing** (M): slice to clips, slice to sampler.
- **Audio quantise** (M): align transients to the grid, with strength.
- **Tempo detection** (M): detect tempo and build a tempo map from a free
  performance.

**Exit criteria**
- A drum loop at a different tempo can be dropped into a project and
  follow tempo changes with no audible artefacts at ±20%.
- A student's free-tempo recording can be turned into a tempo map that the
  metronome follows.

---

## Phase 7 — Intelligence & extensibility

**Goal:** advanced audio analysis, modulation and third-party extension.

- **Monophonic pitch editing** (XL): note-level pitch, drift, vibrato and
  formant editing on vocal/guitar lines.
- **Audio-to-MIDI** (L): monophonic first, then polyphonic. Chord detection
  writes to a chord track.
- **Stem separation** (L): on-device inference (WebGPU/WASM), only with
  models whose weights *and* training data permit our use (decision record
  required). Lesson use: remove the guitar part from a backing track.
- **Chord track** (M): global harmony that guides scale mode, pattern
  sequencer, and note transformations.
- **Note transformations** (M): strum (guitar-specific), arpeggiate,
  ornament, humanise, connect.
- **Melodic pattern sequencer** (M): scales, mono/poly, randomise.
- **Modulators** (M): LFO, step, envelope, envelope follower, random,
  sample & hold. Can drive any parameter.
- **Web plugin hosting** (L): Web Audio Modules 2.0 host, plugin sandboxing,
  preset storage, automation, and delay compensation for hosted plugins.
- **Scripting** (M): sandboxed user scripts for editor actions and MIDI
  processing.
- **MPE / MIDI 2.0** (M): as browser APIs allow.

**Exit criteria**
- A third-party WAM plugin can be installed, automated, saved and
  collaborated on.
- Pitch-edited and stem-separated audio renders identically offline and
  in real time.

---

## Phase 8 — Optional desktop wrapper (Electron)

**Goal:** the *same* app, packaged for desktop, with native-quality audio
I/O and native plugins. This phase is **optional** and can start any time
after Phase 2, but it must never fork the codebase.

- **Shell** (M): Electron app loading the web build, with auto-update and
  code signing (macOS notarisation, Windows signing).
- **Native audio backend** (L): a native audio module (C++ or Rust) behind
  the *same engine interface* the web build uses. ASIO (Windows),
  CoreAudio (macOS), JACK/PipeWire/ALSA (Linux). Exclusive/low-buffer
  modes.
- **Low-latency software monitoring** (M): through effects, which makes
  Live Looper fully usable for guitar.
- **Native plugin hosting** (XL): VST 3 (MIT SDK) and CLAP, in a separate
  plugin process for crash isolation. Plugin UIs in native windows.
  State is serialised into the shared project so web collaborators can
  see (but not run) them, with a frozen-audio fallback.
- **Licensing** (S): ASIO SDK is GPLv3 *or* proprietary. Choose per
  decision 0001 and record it.
- **Filesystem** (S): projects on local disk, watched folders, large
  sample libraries.

**Exit criteria**
- Round-trip input-to-output latency ≤ 10 ms on Windows with ASIO at a
  64–128 sample buffer on reference hardware.
- A project made on desktop with a native plugin opens on the web, using
  frozen audio for that plugin's track.

---

## Phase 9 — Professional scale

**Goal:** match the depth expected of a workhorse DAW in large
productions.

- Large projects: 200+ tracks, long sessions, lazy loading, multi-core DSP
  (multiple worklets / WASM threads), a performance profiler in the app.
- **Score/notation editor** (XL), building on tab view.
- **Articulation/expression maps** (M) for orchestral libraries.
- **Microtonal tuning** (M) via scale files.
- **Video track** (L) for scoring and sync. Timecode, frame rates.
- **Surround/immersive mixing** (XL): multichannel buses, panning, binaural
  monitoring. Format licensing reviewed before any branded format support.
- **Spectral view and repair** (L).
- **Constrained generators** (M): rule-based rhythm/melody/chord
  generation within scale/chord constraints.
- **External hardware integration** (M): MIDI clock/MTC sync, external
  instrument and effect routing with latency compensation (desktop).
- **Accessibility depth**: complete screen-reader workflows for blind and
  low-vision musicians.
- **Localisation**.

---

## Cross-cutting tracks (every phase)

| Track | Standing commitments |
|---|---|
| **Audio quality** | Golden-render tests, null tests for routing/latency, dropout monitoring in CI on a reference profile, fuzzing of file decoders. |
| **Performance** | Budgets per phase (tracks/effects before dropout; UI frame time at 60 fps with large projects). Regressions fail CI. |
| **Collaboration correctness** | Randomised multi-client convergence tests on the CRDT schema; schema migrations versioned and tested. |
| **Security & privacy** | Least-privilege storage access, signed asset URLs, rate limits, child-safety review for lesson features, dependency audits. |
| **Accessibility** | Keyboard-first design, ARIA, reduced-motion, contrast. Screen-reader testing each phase. |
| **Clean-room hygiene** | Decision records, licence checks, trademark-free naming, periodic review of [CLEAN_ROOM.md](CLEAN_ROOM.md). |
| **Docs** | User guide and developer docs grow with each phase. All original writing. |
| **User research** | Every phase is validated with real musicians. Phase 1 to 4 always includes a teacher and a student. |

---

## Dependency map

```
Phase 0 ─► Phase 1 (Lesson Room) ─► Phase 2 (Core audio DAW)
                                     │
                     ┌───────────────┼──────────────────┬───────────────┐
                     ▼               ▼                  ▼               ▼
              Phase 3 (MIDI)   Phase 4 (Collab)  Phase 5 (Looper)  Phase 8 (Desktop, optional,
                     │                                  │           can run in parallel)
                     └───────────────┬──────────────────┘
                                     ▼
                       Phase 6 (Clip Grid & stretch)
                                     ▼
                       Phase 7 (Intelligence & plugins)
                                     ▼
                       Phase 9 (Professional scale)
```

- Phase 5 needs Phase 2 (effects, mixer) and benefits from Phase 3 (MIDI
  control). It can start once 3.1 MIDI foundation is done.
- Phase 6 Clip Grid reuses the Phase 5 scheduler.
- Phase 8 can start after Phase 2 and run in parallel with everything after.

## Open decisions

Tracked in [`docs/decisions/`](decisions/). Status as of this roadmap:

| # | Decision | Status |
|---|---|---|
| 0001 | Project licence (open source vs proprietary vs dual) | **Open — owner to decide in Phase 0** |
| 0002 | Web-first with optional Electron wrapper | Accepted |
| 0003 | DSP language: Rust → WASM | Proposed — confirm in spike 0.3 |
| 0004 | Collaboration model: CRDT (Yjs) + object storage for assets | Proposed — confirm in spike 0.5 |
| 0005 | Timing model: tick-based musical time + sample-accurate engine | Proposed |
| — | Product name and trademark search | Open — before public launch |
| — | Hosting provider (must support COOP/COEP headers) | Open — Phase 0 |
| — | UI framework | Open — Phase 0 (see Architecture §7) |
