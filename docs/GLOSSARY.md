# Glossary

Our vocabulary for code, UI and docs. Terms are **generic industry terms** or
**our own names**. Do not introduce trademarked feature names from other
products (see [CLEAN_ROOM.md](CLEAN_ROOM.md) §2). Names marked *(working)* are
placeholders that may change before launch.

## Core model

| Term | Meaning |
|---|---|
| **Project** | One song/session/lesson document: tracks, clips, mixer, tempo map, comments. The unit of sharing and sync. |
| **Track** | A horizontal lane holding clips and owning a mixer channel. Kinds: audio, instrument (MIDI), bus, folder, looper. |
| **Clip** | A placed region on a track referencing source material (audio asset or note data) with offset, length, gain, fades. |
| **Asset** | An immutable media file (e.g. a recorded take). Clips reference assets; assets are never edited in place. |
| **Take** | One recording pass. Multiple takes over the same time range stack in **take lanes**. |
| **Comp** | A composite clip assembled from parts of several takes. |
| **Timeline** | The linear, time-ordered view of a project (often called "arrangement" generically). |
| **Clip Grid** *(working)* | The non-linear view where clips sit in slots and are launched in time with the transport. |
| **Scene** | A row of the Clip Grid launched together. |
| **Live Looper** *(working)* | The performance mode for live loop recording/overdubbing with per-track loop controls and effects. See Roadmap Phase 5. |
| **Loop Track** | A track in Live Looper mode with record / overdub / play / stop state. |
| **Phrase Set** *(working)* | A saved, recallable set of loop tracks plus their settings in Live Looper mode. |
| **Transport** | Play / stop / record / position / loop region / tempo. |
| **Tempo Map** | Tempo and time-signature changes over the timeline. |
| **Musical Time** | Positions in bars/beats/ticks (PPQ). **Absolute Time** = seconds/samples. |
| **Marker** / **Section** | Named point / named range on the timeline (e.g. "Verse 1"). |

## Audio

| Term | Meaning |
|---|---|
| **Engine** | The real-time audio processing graph running off the main thread. |
| **Device** / **Processor** | A unit in a channel's effect/instrument chain (our own or plugin). |
| **Insert** | A processor placed in series on a channel. |
| **Send** | A tapped copy of a channel's signal routed to a bus. |
| **Bus** | A channel that sums other channels (group bus, effect/return bus). |
| **Control Group** *(working)* | A fader that controls the relative levels of a set of channels without summing audio (generically, "VCA-style"). |
| **Sidechain** | Feeding one channel's signal to a processor on another. |
| **Latency Compensation** | Aligning signals delayed by processing (plugin delay) or by hardware (round-trip/record offset). |
| **Freeze** / **Render in Place** | Rendering a track to audio to save CPU, reversibly / permanently. |
| **Stretch** | Changing duration without changing pitch. **Shift** = changing pitch without changing duration. |
| **Stretch Markers** *(working)* | Anchors placed on audio that can be moved to retime it. |
| **DI** | Direct-input (dry, unprocessed) instrument signal. |
| **Monitoring** | Hearing the input while playing. *Direct/hardware* monitoring happens on the interface; *software* monitoring passes through the engine. |

## Collaboration

| Term | Meaning |
|---|---|
| **Room** *(working)* | A shared space around a project with members and roles. A **Lesson Room** is a Room set up for teacher + student(s). |
| **Member Roles** | Owner, Editor, Commenter, Viewer (lesson defaults: teacher = Owner, student = Editor). |
| **Presence** | Live indication of who is in a project, where their playhead/selection is. |
| **Pinned Comment** | A comment attached to a time position/range on a track or the timeline. |
| **Snapshot** | A named, restorable version of a project. |

## MIDI & composition

| Term | Meaning |
|---|---|
| **Note Editor** | Pitch-over-time grid editor for note data (generically, "piano roll"). |
| **Step Sequencer** | Grid of steps per sound, for pattern-based programming. |
| **Pattern** | A reusable block of note/step data. |
| **Chord Track** | A global track describing the harmony over time, usable by tools. |
| **Scale Mode** | Project/clip key + scale used to guide editing and generation. |
| **Modulator** | A source (LFO, envelope, step, random, follower) that drives parameters. |
| **Tab View** | Guitar/bass tablature view of note data. |
