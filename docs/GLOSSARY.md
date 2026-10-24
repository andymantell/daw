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
| **Monitoring** | Hearing the input while playing. *Direct/hardware* monitoring happens on the interface; *software* monitoring passes through the engine (and its effects). |
| **Cue Mix** | A separate headphone mix for a performer. |
| **Talkback** | A microphone path from the control position into performers' cue mixes. |
| **Plugin Host** | The separate process(es) that load third-party plugins (VST 3, CLAP, AU), isolating crashes from the app. |
| **Frozen Audio** | A rendered audio copy of a plugin track, shared with the project so collaborators without that plugin still hear it. |

## Collaboration

| Term | Meaning |
|---|---|
| **Lesson Room** *(working)* | A project set up for a teacher and student(s), with members and roles. The name of the Phase 1 vertical slice. |
| **Member Roles** | Owner, Editor, Commenter, Viewer (lesson defaults: teacher = Owner, student = Editor). Members are identified by public keys; there are no server accounts. |
| **Live Session** | A real-time peer-to-peer connection. One member (usually the teacher) hosts, and the others join with an **Invite Code**. |
| **Bundle** | A project folder packed into one file for sending. Importing it merges into an existing copy. |
| **Shared Folder** | A project folder kept in a cloud-synced directory. The app merges changes from other members. |
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
| **Notation Lane** | A timeline lane showing a track's notes as notation (tab, standard staff or drum staff), synced to the tempo map and playback. |
| **Tab View** | Guitar/bass tablature view of note data, per string, with techniques. |
| **Tuning** | String count and pitch per string (plus capo) for a tab track. |
| **Drum Staff** | Five-line percussion staff view of drum note data. |
| **Drum Map** | Mapping of kit pieces to note numbers, staff positions and noteheads. Defaults to the General MIDI percussion map. |
| **Fret Assigner** | The tool that picks string and fret for notes that arrive without positions. |
