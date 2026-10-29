# Roadmap

**Vision:** a native desktop digital audio workstation for Windows, macOS
and Linux. It starts as the best tool for **guitar lessons**, where teacher
and student share projects, and grows phase by phase into a **full
professional DAW** with first-class **VST 3 plugin support** and a visible,
timeline-aware **Live Looper** mode. It needs **no backend server**: all
data lives on the users' machines, and collaboration is peer-to-peer.

Guiding principles:

1. **Vertical slices, not layers.** Every phase ends with something a real
   person can use end to end. The first users are a guitar teacher and a
   student.
2. **Architecture for the destination, scope for the slice.** Phase 1 is
   small, but its project model, engine, plugin host and sync layer must
   not need replacing to reach the final phase.
3. **Native and low-latency.** Direct access to ASIO, CoreAudio,
   WASAPI, ALSA, JACK and PipeWire. Monitoring through software effects
   (e.g. an amp simulator) is a supported, everyday workflow.
4. **Plugins are first-class.** VST 3 hosting arrives in the vertical slice.
   CLAP, Audio Units (macOS) and ARA follow.
5. **No server we operate.** Projects are folders on disk. Collaboration is
   peer-to-peer (teacher hosts, student joins) plus project bundles and
   shared folders.
6. **Collaboration is core.** The project document is conflict-free and
   mergeable from day one.
7. **Clean-room, original work.** See [CLEAN_ROOM.md](CLEAN_ROOM.md).
8. **Audio correctness is non-negotiable.** No dropouts, sample-accurate
   timing, deterministic offline renders, automated audio tests.

How this document works:

- Phases are **sequential milestones**. Later phases may start in parallel
  once dependencies are met (see the dependency map).
- Each phase lists **goals, deliverables, exit criteria and risks**. A phase
  is done when its exit criteria are met.
- **No calendar estimates**. Sizes (S/M/L/XL) are relative effort only.
- Feature coverage is cross-referenced in
  [research/CAPABILITY_LANDSCAPE.md](research/CAPABILITY_LANDSCAPE.md).

---

## Phase 0 — Foundations & the Rust go/no-go spike

**Goal:** settle the technology choice with evidence, retire the biggest
risks, and set up a repository where quality checks are automatic.

### 0.1 Rust go/no-go spike (L) — *first, and decisive*
See [decisions/0003-implementation-language.md](decisions/0003-implementation-language.md).
Build throwaway prototypes in Rust to prove four things:

| Test | Pass criteria |
|---|---|
| **A. Audio I/O** | Full-duplex audio via ASIO (Windows) and CoreAudio (macOS) at 64–128-sample buffers, 48 kHz. 30 minutes with no dropouts under a synthetic DSP load. Measured round-trip latency recorded. Linux (JACK/PipeWire) best-effort. |
| **B. VST 3 hosting** | Load **10–15 widely used VST 3 plugins** (free and commercial; effects and instruments; at least two amp simulators; at least one with copy protection) on Windows and macOS. Each one must: process audio; report latency; take parameter changes and automation; save and restore its state; and open, resize and close its editor window embedded in our own window. Unexpected crashes are logged. Out-of-process hosting is prototyped for at least two plugins. |
| **C. UI** | In one or two candidate toolkits, a timeline with 300+ clips with waveforms, smooth horizontal and vertical scroll and zoom at 60 fps on a mid-range laptop. Text entry, menus, drag-and-drop and keyboard shortcuts work. A screen reader can read basic controls. |
| **D. Engine** | A lock-free command path from UI to audio thread, and a graph swapped atomically without glitches. No allocations on the audio thread, verified with tooling. |

**Decision rule:** if A–D pass, adopt Rust. If **B or C** become a slog, with
plugin windows or the UI toolkit eating disproportionate time, switch to
**C++ with JUCE** before real product code exists. Then re-run B and C
quickly to confirm. The result goes in decision 0003.

### 0.2 Repository & tooling (S) — *started 2026-10-27*
- Workspace layout per [ARCHITECTURE.md](ARCHITECTURE.md) §10.
- CI on **Windows, macOS (Apple Silicon and Intel) and Linux**: build,
  formatting, lints, unit tests, dependency licence check, and real-time
  safety checks (no allocation on the audio thread) where tooling permits.
- Contribution rules: DCO sign-off, code of conduct, clean-room policy.
- Decision records in `docs/decisions/`.

### 0.3 Project licence (S) — *done*
- **GPL-3.0-or-later**, see
  [decisions/0001-project-licence.md](decisions/0001-project-licence.md).
  The ASIO SDK is used under its GPLv3 option, and GPL-compatible
  stretch libraries are allowed.

### 0.4 Latency & recording-offset spike (M)
- Measure the true round-trip latency with a loopback cable. Compare it
  with what the driver reports, per OS and driver.
- Record a guitar DI against a click. After compensation, takes must land
  within ±1 sample-block of the grid.
- Status (2026-10-27): loopback measurement tool built
  ([spikes/latency](../spikes/README.md)); waiting for a run on the test
  machine.

### 0.5 Peer-to-peer collaboration spike (L)
- Prototype the project document on a Rust CRDT (candidates: `yrs`,
  `automerge`, Loro). Two instances edit tracks and clips at the same time.
  Offline edits merge on reconnect.
- Prototype **teacher-hosted sessions with no server we operate**. Compare:
  WebRTC data channels (STUN-based hole punching), QUIC with UPnP/NAT-PMP
  port mapping, and a manual invite-code exchange.
- Measure the connection success rate across home broadband, mobile
  hotspot (carrier-grade NAT) and a strict corporate/school network.
  Document the fallbacks.
- See [decisions/0004-collaboration-model.md](decisions/0004-collaboration-model.md).
- Status (2026-10-27): document half done (Loro preferred). Connection
  half built and passing on one machine (QUIC, codes swapped by hand,
  STUN, hole punching); real-network runs are in the
  [review queue](REVIEW_QUEUE.md).

### 0.6 Speed & pitch spike (M)
- Real-time pitch-preserving playback at 50–150% speed, and transpose by
  ±12 semitones. Compare licence-compatible libraries with our own
  implementation from published literature. Listening tests on guitar
  material, plus CPU per track.
- Status (2026-10-27): started. CPU and objective checks done for two
  MIT-licensed libraries and our own phase vocoder
  ([spikes/stretch](../spikes/README.md)). All are light enough per track
  and none allocates on the audio thread. Listening tests on real guitar
  recordings are still to do.

### 0.7 Platform & distribution baseline (M)
- Supported OS versions. Apple Silicon and Intel builds. How to handle
  Intel-only plugins on Apple Silicon (bridge process vs unsupported).
- Code signing: Apple Developer Program membership, macOS notarisation, a
  Windows code-signing certificate. An update mechanism that needs no server
  we operate, such as release files hosted on GitHub Releases.
- Crash reports written to local files that the user can choose to send.
  No telemetry server.
- *Status (2026-10-27): research written up in
  [PLATFORM_DISTRIBUTION.md](research/PLATFORM_DISTRIBUTION.md), with
  proposals waiting on Andy in [REVIEW_QUEUE.md](REVIEW_QUEUE.md).*

**Exit criteria**
- Decision 0003 is accepted (Rust or JUCE), with spike evidence attached.
- All spikes have written results in `docs/decisions/`.
- CI is green on all three operating systems, with the licence check enforced.

**Key risks:** plugin-window embedding edge cases; UI toolkit maturity; NAT
traversal without a relay; stretch quality vs CPU.

---

## Phase 1 — Vertical slice: **Lesson Room**

**Goal:** a teacher and a student run real guitar lessons with it, in
person or remote. Between lessons, they record ideas, practise against
backing tracks at reduced speed through their favourite amp-sim plugin,
and leave each other time-pinned feedback in a shared project.

**Primary user stories**
- *Teacher* creates a lesson project, imports a backing track, records an
  example riff through a VST 3 amp simulator, marks sections ("Intro",
  "Bars 9–16"), and invites the student.
- *Student* joins (live via invite code, or by opening a shared bundle or
  folder). They loop "Bars 9–16" at 70% speed with the teacher's riff
  muted, record three takes while monitoring through their own amp plugin,
  and pin a question at bar 12.
- *Teacher* reviews the takes, pins feedback on take 2, and records a
  corrected version. The student receives the changes on the next sync.
- Both use a tuner and metronome and can work fully offline.

### 1.1 Engine core (L)
*Started 2026-10-27, at risk until the Phase 0 checks on the owner's PC
pass. Done so far: tempo map, render graph with track mixer and meters,
sample-accurate transport and loop, metronome with count-in and a
separate click output, clips streamed from WAV files on disk, offline
render (`crates/engine`), audio devices (`crates/audio-io`), and a first
app window with a mixer, transport and device settings (`crates/app`).
Files at other sample rates are converted on import, in the background,
into a cache folder (to move into the project folder once projects
are saved). If the device is unplugged, the app plays silently and
starts again on it when it is back. Measured latency: Settings → Audio
device → Recording offset → Measure… plays test chirps through a loopback
cable, in the engine's place and through the same input queue, over three
starts of the audio, and saves the round trip as the device profile's
recording offset, noting the rate and buffer size it was measured at
(simulated in the cloud; the owner's cable test is in the review queue).*
- Audio device selection (driver, device, sample rate, buffer size) and a
  device-settings panel with measured latency.
- Transport: play, stop, position, loop region, tempo, time signature
  (single tempo in this phase).
- A sample-accurate scheduler. Conversion between musical and absolute time.
- Audio clip playback with disk streaming. Per-track gain, pan, mute,
  solo. Master bus. Peak meters.
- Metronome: accents, subdivisions, count-in, separate output routing.
- Offline render path, sharing code with real time. It powers export and
  tests.

### 1.2 Recording & monitoring (L)
*Started 2026-10-27, at risk. Done so far: per-track input (mono or
stereo pair), arm and software monitoring; Record with count-in, starting
on the exact frame; one take file per pass, checkpointed every second so
a crash keeps what was captured; takes kept per track, newest playing,
older ones switchable; the recording offset per device profile (measured,
or from the driver's reported latency); device input passed to the
engine through a lock-free queue. Until a project is saved, takes go in
the user's profile under `DAW/Unsaved takes` (owner's choice,
2026-10-27; `DAW` becomes the product's name once it has one, set in
one place in the app); saving the project moves them into its folder
(to build with project saving). Device settings are kept between runs, per device
(`%APPDATA%\DAW\settings.txt`), so each interface keeps its own recording
offset; a direct-monitoring option stops the app playing inputs back
when the interface's own monitoring is used.*
- Input channel selection per track, arm, record, count-in.
- **Low-latency software monitoring** through the track's effect chain,
  with a direct-monitoring option.
- Recording-offset compensation (from 0.4), stored per device profile.
- Takes are written straight to the project folder. Crash-safe: an
  interrupted recording keeps what was captured.
- Several takes per track: newest on top, older ones kept and switchable.

### 1.3 Plugin hosting — first cut (L)
*Started 2026-10-28, at risk. Done so far: VST 3 scanning of the
standard folders, each bundle opened by a separate copy of the app with a
30 s limit, results kept in `%APPDATA%\DAW\plugins.txt` and only changed
bundles rescanned; a plugin list (Settings → Plugins) to switch plugins
on and off, rescan, and retry failures; per-track effects (FX button) to
add, bypass, reorder and remove, with the host's own sliders for any
plugin's parameters and, on Windows, the plugin's own editor window;
latency compensated by reading the track's clips ahead; monitoring runs
through the effects; effects carry over unchanged when the session
changes, and are made again with the same settings when the device or
sample rate changes. Effects and their settings are saved in the
project (1.6). Still to do: frozen audio for missing plugins (with
collaboration). Known limits: effects run inside the app, so a
plugin that crashes while playing takes the app with it.*
- Plugin scanning, run out of process so a bad plugin can't crash the app.
  A plugin manager lets users enable, disable and rescan.
- **VST 3 effects as track inserts** (instrument plugins come in Phase 3),
  with bypass, reorder and saved state.
- Plugin editor windows. Parameter changes saved in the project.
- Plugin latency reported and compensated on the track.
- **Missing plugin handling:** if a collaborator lacks a plugin, the track
  plays its **frozen audio** (rendered automatically and shared with the
  project), and the plugin's state is kept intact.

### 1.4 Practice tools (M)
*Started 2026-10-28, at risk. Done so far: a chromatic tuner
(Practice → Tuner) on any input channel, reading within a cent from a
five-string bass's low B up, with the reference pitch setting (A = 400 to
480 Hz, saved in the project); a per-track practice mute (🎧), kept apart from the mix's own
mute; speed 50–150% and transpose ±12 semitones plus cents, with tuning
presets, through our own phase vocoder from spike 0.6 (a library can
replace it if the 0.6 listening prefers one). Tracks stay in time with
the transport, the metronome follows the slowed beat, and loops wrap
smoothly; nothing records while speed or pitch is changed. A tempo
trainer raises the speed by a step every N loops up to a target.
Markers name sections on the timeline, and a section can be looped from
its marker's menu (1.5). Speed, transpose, practice mute, loop on/off
and the tempo trainer are saved in the project, each computer keeping
its own so one person's practice doesn't change another's (the loop's
bars stay the song's, shared). Tuning presets can measure a track's
offset from concert pitch (spectral peaks against the tuner's
reference) and offer the fine-pitch shift that corrects it; whole-step
tunings such as E♭ read as in tune, so those stay with the presets.*
- Loop region, chosen quickly from sections and markers.
- Pitch-preserving speed (50–150%) and transpose (±12 st), plus fine pitch
  in cents. **Tuning presets** for common shifts (e.g. "half step down →
  standard" = +1 st), and a **detected tuning offset** from the reference
  pitch that suggests the shift. Works on any audio clip, including the
  audio of an imported video file.
- Tempo trainer: speed increases by X% every N loops up to a target.
- Chromatic tuner. Reference pitch setting.
- Per-track "practice mute".

### 1.5 Editing — minimal set (M)
*Started 2026-10-28, at risk. Done so far: a timeline above the mixer
with each track's clips and waveforms; select, move, trim, split and
delete, snapped to beats or bars (Alt places freely); markers on bar
lines, renamed by double-click, that name sections and loop them; zoom
and scroll; recorded takes as clips, a new take muting older takes under
it. Undo/redo now goes through the project document (1.6). Clips stay on their own track for now. Import opens WAV, MP3, FLAC, OGG Vorbis, AAC/M4A, AIFF, ALAC and CAF, and the sound of MP4, MOV and MKV videos (AAC sound copied out as it is, so a project keeps only the audio); surround is mixed to stereo.*
- Select, move, trim, split, delete clips. Snap to beat or bar.
- Import audio files (drag and drop).
- Markers and named sections.
- Undo/redo that works correctly with merged collaborator edits.

### 1.6 Collaboration — serverless (L)
*Started 2026-10-28 after decision 0004 was accepted. Done so far: a
project is a folder holding `project.daw` (a Loro document, with a
`.bak` of the previous save) and `Audio/`, each file named by the hash
of its content. Saving copies imported files in and moves takes in from
`Unsaved takes`. Undo and redo are the document's own, per user, so
they will work with merged edits. Bundles: File → Export bundle… writes
one `.dawbundle` file (a plain tar of the project folder: the document
and its audio), and importing one
merges it into the user's copy of that project, or makes a new project.
Shared-folder sync: each computer writes its own copy of the document
into the folder's `Devices` folder within seconds of an edit and merges
other computers' copies as they arrive. Comments: Practice → Comments…
or right-click the timeline to pin a comment to a time or a stretch of
time on a track or the whole song, with replies, resolve and play from
there; pins show on the timeline. Deleting a comment, or changing its
text, blanks the old words out of the saved history, the backup and
bundles. Still to do: live sessions and identity (comments show "You" or
"Someone else" until then).*
- **Project = a folder on disk**: a document file plus content-addressed
  audio assets.
- **Project bundle** export and import: a single file to send by any means.
  Importing a bundle of an existing project *merges* it (CRDT) rather
  than overwriting.
- **Shared-folder sync:** if the project folder lives in a user's
  cloud-synced folder (any provider), the app detects changes and merges
  them safely.
- **Live session (teacher hosts):** the teacher starts a session and shares
  an invite code. The student joins peer-to-peer. Edits sync in real time,
  with presence (who's here, their playhead) and **follow mode** (the
  student's view follows the teacher's playhead).
- Asset transfer peer-to-peer: chunked, resumable, verified by hash.
- **Pinned comments:** on a time position or range on a track or the
  timeline, with threads, resolve, and jump-to-play.
- Identity without accounts: each install has a local key pair and a
  display name. Members are known by their public keys, and the project
  owner grants editor or commenter roles.

### 1.7 Lesson notes & tab (S)
*Started 2026-10-28: Practice → Lesson notes… keeps plain-text notes in
the project document. Rich text and tab blocks are still to do.*
- Rich-text lesson notes per project. Plain-text tab blocks pinned to
  sections.

### 1.8 Output, safety & distribution (M)
*Started 2026-10-28. Done so far: File → Export writes the mix, and
optionally each track as a stem, as 24-bit WAV at the session's rate,
for the whole song or the loop, into a new numbered folder in the
project's `Exports` folder (`DAW\Exports` in the user's profile before
it is saved, or a folder the user chooses). Rendering runs in the background
with its own copies of any plugins; practice mute, live input and the
metronome are left out, and clipping is reported. A saved project is
autosaved every 30 seconds. No crash recovery yet for a project that
was never saved.*
- Export mixdown as WAV, plus per-track stems.
- Snapshots ("Lesson 3 end") and restore. Autosave. Crash recovery.
- Signed, notarised installers for Windows and macOS. Linux build
  (AppImage or Flatpak, chosen in 0.7). Update notification.
- Privacy: everything stays local by default. Lesson users may include
  **children**, so review applicable child-data rules (e.g. UK Age
  Appropriate Design Code / GDPR, COPPA in the US) before public release,
  even without a server. *Reviewed 2026-10-28 in
  [research/PRIVACY_CHILD_DATA.md](research/PRIVACY_CHILD_DATA.md), with
  design rules for 1.6 identity, live sessions and comments, and decisions
  in [REVIEW_QUEUE.md](REVIEW_QUEUE.md). The app is for the owner and their
  teacher and not promoted to schools, so these are good practice;
  revisit if that changes or a hosted service is added.*

### 1.9 Quality bar (M, runs throughout)
*Started 2026-10-28. The app measures its DSP load (time in the audio
callback against the buffer's length) and counts late buffers; Settings
→ Performance check adds 16 generated stereo tracks and an effect on 8 of
them to try the bar on real hardware. Golden-render tests compare nine
short mixes (metronome, loop, a 44.1 kHz clip, speed and transpose, a
tempo change) with reference renders kept in the repository. The
plugin host is tested against our own test plugin and broken builds of
it that crash or hang while loading, refuse to set up, or get damaged
state, and an effect's settings are checked through a saved project.
Scripted merge tests run people's edits through live updates, bundles
(late and repeated) and a shared project file, plus 100 random scripts,
checking that every copy agrees and nothing is lost. Every control has
a screen-reader name, checked by a test that reads the accessibility
tree; the transport works from the keyboard (Space, Home, R, L, K, C,
Tab and Enter), listed under Help → Keyboard shortcuts. Every clip and
marker on the timeline is an entry in that tree too, named with its
track and bars ("Take 2 on Guitar, bar 3 to bar 5"), reached with Tab,
and edited with the arrow keys: move by a beat or bar, trim either
edge, move to the track above or below, select, split and delete; a
marker is moved, renamed, looped and deleted the same way. Clips can
also be dragged to another track with the mouse. Known limit: the
merge scripts run on the document, not yet through a real network
session.*
- No dropouts with 16 stereo tracks plus 8 plugin instances at a
  128-sample buffer on the reference laptop.
- Golden-render audio tests. Automated plugin-host tests with a set of
  freely redistributable test plugins.
- Accessibility: full keyboard operation of transport and recording, and
  screen-reader labels.

**Exit criteria**
- A real teacher and student complete **three consecutive lessons** and the
  practice between them using only this app, with no blocking bugs.
- Recorded takes land within ±1 ms of the grid after calibration on the
  supported OS/driver matrix.
- Concurrent and offline edits merge without loss in the scripted
  conflict tests, via live session, bundle and shared folder.

**Out of scope for Phase 1:** MIDI, instrument plugins, automation, buses,
tempo changes, comping UI, built-in effects beyond gain/pan/tuner, and
video calls.

---

## Phase 2 — Core audio DAW

**Goal:** record and mix a full song of guitar, bass, vocals and drums
from audio.

### 2.1 Editing depth (L)
- Fades and crossfades with curve shapes. Clip gain and gain envelopes.
- Nudge, slip-edit, ripple edit, range tools, grid and snap modes, zoom.
- Group editing across tracks (edit multiple drum mics together).
- Customisable key commands with a searchable command palette.
- Offline processing (normalise, reverse, gain) creates new assets.
- **Fix a badly timed take, first cut** (M): detect attacks (transients) in
  a clip, using onset-detection methods from published literature, and show
  them as markers with a sensitivity control. Then **slice and move**: cut
  at each attack and snap the slices to the grid, either all at once (with
  strength, so a take can be tightened without becoming robotic) or by
  dragging single attacks onto the beat. Crossfades cover the gaps. No time
  stretching, so it suits percussive parts best. Works across grouped
  tracks so multi-mic drums stay in phase. Non-destructive and undoable.

### 2.2 Recording depth (M)
- Punch in/out (manual and range-based), pre-roll and post-roll.
- Loop recording into stacked **take lanes**.
- **Comping:** swipe to choose the best parts, automatic crossfades,
  audition takes, flatten a comp. Save **several comps** of the same takes
  and switch between them.
- Multi-input recording on many armed tracks.
- Retrospective capture: keep recent input even when not recording.
- **Remote control app for Android** (M): record and run the transport from
  a phone while away from the computer, for example at a drum kit.
  - Controls: record, play, stop, return to start, arm tracks, count-in and
    metronome on/off, undo last take. Large buttons that work with sticks
    in hand, haptic feedback, landscape and portrait.
  - **Timeline overview:** tracks, clips, markers, loop region and playhead.
    Tap or drag to move the playhead and recording start point, snapping to
    bars and markers. Set a punch-in range.
  - Input level meters and clip warnings, to check mic levels from the kit.
  - Keeps the screen on while open (a standard window flag on Android needs
    no extra permission).
  - Local network only, paired by QR code. No internet or server of ours.
    The desktop stays in charge: losing the phone's connection never stops
    or spoils a recording, and the app reconnects by itself.
  - Why an app and not just a web page served by the desktop: a page on
    plain HTTP over the local network is not a "secure context", so the
    browser will not let it keep the screen awake. See
    [research/REMOTE_CONTROL.md](research/REMOTE_CONTROL.md).
  - Uses the same control protocol as the Phase 5 companion remote, which
    later adds looper pages to this app.

### 2.3 Mixer, routing & monitoring (L)
- Inserts, sends (pre/post fader), effect return buses, group buses,
  folder tracks.
- Control-group (VCA-style) faders. Sidechain inputs.
- **Engine-wide delay compensation**, including sends, sidechains and
  plugins.
- **Monitor section:** separate control-room output, **cue/headphone mixes**
  per performer, **talkback**, dim/mono, speaker-set switching. This is
  great for in-person lessons: the student gets their own headphone mix.
- Metering: peak/RMS, LUFS, true peak, spectrum, phase correlation.
- Mixer view inside the main window, and as a full window.

### 2.4 Plugin hosting — depth (L)
- Sandboxing options per plugin: in-process, shared host process, or own
  process.
- Multi-output plugins, sidechain inputs to plugins, bus layouts.
- Plugin presets in our own format plus the plugin's native presets.
- Plugin delay changes handled live.
- Plugin editor windows scale with the display. An optional cap on open
  plugin editors protects CPU.
- When a project references an older plugin version (e.g. VST 2) and only
  the newer version is installed, map to the newer one where the plugin
  declares compatibility, keeping state and parameters.
- Compatibility test matrix maintained in CI where licences allow, plus a
  public compatibility list.

### 2.5 Built-in effects, set 1 (L)
- EQ, compressor, gate, limiter, algorithmic and convolution reverb, delay.
- Guitar amp and cabinet simulation (our own models and impulse
  responses). Tuner as an insert.
- **Neural amp model player:** loads open-format neural amp/pedal captures
  (`.nam` files from the open-source Neural Amp Modeler ecosystem), so
  teachers and students can use the tone of a captured real amp without a
  plugin. Built on a licence-compatible inference library (C++ core
  reported as MIT, licence still to verify; pure-Rust ports exist). Users
  supply their own capture files. We bundle only captures we make ourselves
  or that are licensed for redistribution.
- Built on the same processor interface as hosted plugins.

### 2.6 Automation (M)
- Automation lanes for every parameter, including plugin parameters.
- Read/touch/latch/write modes. Clip-level automation.

### 2.7 Tempo & time (M)
- Tempo map with changes and ramps. Time-signature changes.

### 2.8 Project management (S)
- Snapshots with a diff summary. Project templates (e.g. "Guitar lesson",
  "Band demo").

### 2.9 Workspace: panes and multiple screens (M)
- **One window by default.** The timeline, mixer, editors, browser, plugin
  editors and practice tools are panes inside the main window. They can be
  docked, split, resized, tabbed together, collapsed and shown full-size,
  all without leaving that window.
- **Detach on request:** any pane can be torn off into its own window and
  docked back again, so the app can spread over several screens (e.g.
  timeline on one, mixer on another, plugins on a third). Off by default.
- **Saved layouts:** name and recall arrangements ("Recording", "Mixing",
  "Lesson"), with a key command for each.
- **Layouts belong to the computer, not the project.** Collaborators have
  different screens, so window positions are kept in local settings and
  never synced. The project only remembers which panes were open.
- **Screen changes are safe:** if a saved screen is missing (laptop
  undocked, monitor off), its windows come back onto a screen that exists,
  and the layout returns when the screen does. Each window follows its
  own screen's scaling (high-DPI).
- Keyboard focus, shortcuts and the screen reader work the same in every
  window. Closing a detached window docks its pane back. It never closes
  the project.
- Toolkit support: see [research/WORKSPACE_WINDOWS.md](research/WORKSPACE_WINDOWS.md).

**Exit criteria**
- A 24+-track song with 10+ plugins can be recorded, comped, mixed with
  automation and exported with no dropouts at a 128-sample buffer on the
  reference machine.
- Delay compensation is verified by automated null tests.
- A sloppy multi-mic drum take is tightened to the grid at 70% strength,
  and the drum mics stay in phase.
- On a three-screen setup, the timeline, mixer and a plugin editor each sit
  on their own screen. After the app restarts with one screen unplugged,
  every pane is still reachable, and the layout returns when the screen
  is plugged back in.
- From the drum stool, a phone moves the start point to bar 17, arms the
  drum tracks, records a take with count-in, and stops, with the desktop
  out of reach. Turning off the phone's Wi-Fi mid-take does not affect the
  recording.

---

## Phase 3 — MIDI, instruments & practice depth

### 3.1 MIDI foundation (L)
- MIDI device I/O on all operating systems. Instrument tracks, recording,
  overdub/merge, retrospective MIDI capture.
- Note editor with velocity and CC lanes, quantise (strength, swing),
  humanise, legato, transpose, scale snapping. Select and filter notes by
  time, pitch, velocity, probability or length. Draw per-note expression
  curves (pitch glide, LFO-shaped modulation) for MPE-capable instruments.
- **Scale mode:** project and clip key/scale guiding editing.
- Groove templates: extract a groove from audio or MIDI and apply it.
- Standard MIDI file import/export. MIDI learn.

### 3.2 Instrument plugins & more formats (L)
- **VST 3 instruments**, including multi-output instruments.
- **CLAP** hosting (open SDK, MIT).
- **Audio Units** hosting on macOS.

### 3.3 Pattern & step programming (M)
- Step sequencer / drum pattern editor: swing, per-step probability,
  Euclidean fills, per-step velocity and pitch. Patterns as clips.
- Per-lane length, step resolution and play direction (polymeters, e.g.
  triplet hi-hats over a 16th-note kick). Per-step repeats (ratchets),
  micro-offset, gate, velocity variance. Note-repeat and randomised step
  playback.
- **Linked pattern clips:** several clips can reference one pattern, so
  editing the pattern updates every instance. Any instance can be made
  unique, or converted to plain notes.
- Probability across groups of notes (a whole group plays or doesn't), and
  "play one note of this chord at random".

### 3.4 Built-in instruments & content (L)
- Multi-zone sampler, drum sampler, and a synth of our own design.
- **Drum machine instrument:** pads with several layers each, mixing
  samples with **synthesised drum voices**. Choke/exclusive groups (closed
  hi-hat cuts off open), a per-pad effect chain, per-pad outputs and sends.
  Drives, and is driven by, the step sequencer.
- **Slicer:** slice a loop by transients or grid into a playable
  instrument, with per-slice pitch, filter, pan and reverse.
- Our own sound content: recorded or designed in-house, or licensed for
  redistribution.

### 3.5 Effects, set 2 (M)
- Modulation effects, multi-stage saturation, transient shaper,
  de-esser, multiband dynamics.
- **Device containers:** parallel chains with dry/wet, frequency-split
  chains (per band), mid/side and left/right split chains, and layered
  instruments sharing one set of notes. Containers can hold built-in and
  hosted plugins.

### 3.6 Workflow (M)
- Freeze and render in place. Bounce selection. Bounce a whole group in
  place. Paste a copied clip as rendered audio.
- Projects load samples in parallel. A start screen shows recent
  projects and templates.
- Content browser with tags, search, and preview at project tempo.
- Export FLAC, MP3, Ogg/Opus. Stem export by bus.

### 3.7 Lesson depth (M)
- Practice log: time spent, loop counts, tempo reached per section.
- Lesson history: a snapshot per lesson, plus a timeline of lessons.
- Assignments ("bars 9–16 at 90 bpm by next week"), with progress both
  can see.

### 3.8 Notation lanes: guitar/bass tab and drum staff (XL)
**Goal:** tab and drum notation are first-class timeline items. The aim is
recreating songs: transcribe or import a part, line it up with the original
recording, loop and slow it down, and play along while the notation scrolls.

- **One source of truth:** notation is a *view* of a track's note data, not
  a separate document. Tab, standard staff, drum staff and the note editor
  all show and edit the same notes, and the instrument plays them.
- **Notation lane on the timeline:** shown inside or under an instrument
  track, following the tempo map, bars, sections and loop region. It scrolls
  in sync with playback (page or continuous mode), and can sit directly
  under the audio track being transcribed.
- **Guitar & bass tab:**
  - Any string count (6/7/8-string guitar, 4/5/6-string bass) and any
    tuning (standard, drop, open, custom), plus capo.
  - Optional standard staff above the tab; rhythm shown in tab (stems and
    beams) or hidden.
  - Techniques: hammer-on, pull-off, slide, bend and release (with amount),
    vibrato, palm mute, let ring, dead/ghost notes, natural and artificial
    harmonics, tapping, slap/pop (bass), tremolo picking, strum direction.
  - Chord diagrams and chord names above the staff.
  - **Tab entry by keyboard** (string + fret), and by playing: from MIDI
    guitar controllers (one channel per string), from recorded audio via
    audio-to-MIDI (Phase 7), or from the note editor.
  - **Automatic string/fret assignment** when notes come without positions
    (MIDI, audio-to-MIDI). The assigner minimises hand movement, using
    published fingering-optimisation methods, and lets the user override
    positions per note or lock a hand position.
- **Drum staff:**
  - Standard five-line percussion staff, with a **drum map** (which kit
    piece goes on which line or space, and with which notehead), following
    widely used published drumset-notation conventions. Editable per
    project, with a printed legend (key).
  - Default mapping from the General MIDI percussion key map, so MIDI drum
    parts display correctly with no setup. Custom maps for other kits and
    drum plugins.
  - Articulations: accents, ghost notes, flams, drags, rolls, open/closed/
    pedal hi-hat, cymbal chokes, rimshots and cross-stick, with voicing
    (hands up, feet down).
  - Entry by drum pad / e-kit (MIDI), the step sequencer, or the note editor.
- **Playback & practice tie-ins:** everything from Phase 1 practice tools
  applies (loop a bar range, slow down, tempo trainer). Count-in and the
  metronome follow the notation's bars. Practice-mute the transcribed part
  to play it yourself.
- **Import:**
  - **MusicXML** (open W3C Community Group format, v4.0), including tab
    and percussion parts.
  - **Standard MIDI files**, with drum-channel detection and the General
    MIDI drum map. Tab positions are assigned automatically for guitar and
    bass tracks.
  - **Plain-text (ASCII) tab**, best-effort parser with a review step.
    Rhythm is often missing, so the user aligns it to the beat grid.
  - Proprietary tab-editor formats are **not** supported unless an official
    specification becomes public (see decision 0009).
- **Export:** MusicXML, MIDI, plain-text tab, and print/PDF of the notation
  with the drum legend.
- **Rendering:** our own layout engine, using a **SMuFL**-compliant music
  font (the standard music-font layout). SIL OFL-licensed fonts can be
  bundled with their licence.

### 3.9 Play-along video clips (L)
**Goal:** video is a first-class timeline clip for playing along and song
recreation. See [research/VIDEO_STEMS_REPITCH.md](research/VIDEO_STEMS_REPITCH.md).
- **Video files** the user has the right to use: a video clip on the
  timeline with a floating or docked picture. Its audio is an ordinary
  audio clip, so speed, transpose, tuning presets, looping and (from
  Phase 7) stem separation all apply.
- **Online video links** as *reference* clips: the platform's own visible
  embedded player, synced to the transport (play, pause, seek, loop) with a
  manual offset. Within the platform's terms, only the speeds its player
  supports are offered. No re-pitching, stems, mixer routing, caching or
  offline playback. Collaborators get the link and offset, and each
  streams the video themselves.
- Needs an embedded web view (depends on decision 0007). A spike measures
  sync accuracy between an embedded player and the engine.

**Exit criteria (Phase 3)**
- A song can be composed with built-in and third-party instruments.
- A teacher can set an assignment and see the student's practice progress.
- A student drops in a video file of a song, plays along at 75% speed after
  shifting it +1 semitone to standard tuning, and an online video link
  stays in sync with the transport as a reference.
- A student imports a MusicXML or MIDI file of a song, sees guitar tab, bass
  tab and a drum staff lined up under the original recording, and loops and
  slows a section while the notation follows.
- Tab and drum notation round-trip through MusicXML without losing notes,
  string/fret positions or supported techniques.

---

## Phase 4 — Collaboration depth & interchange

- **Live lesson call** (L): peer-to-peer audio and video in a session. Music-quality
  audio (high-bitrate stereo Opus, no voice processing). The teacher can
  send their playback to the student.
- **Connectivity hardening** (M): better NAT traversal, connection
  diagnostics, and clear fallbacks when a direct connection is impossible.
  An optional relay that **users host themselves** is an open question
  (see decision 0004).
- **Review mode** (M): proposals on a branch of the project, with diff and
  accept/reject.
- **Track versions** (M): alternative versions of a track's content and
  settings.
- **Interchange: DAWproject** (L): import and export of the open DAWproject
  format (v1.0, MIT-licensed; a ZIP of XML plus audio and plugin-state
  files). It's supported by several major DAWs, so teachers and students
  can move songs in and out of other DAWs.
  - Our own reader/writer, implemented from the published schema and
    reference (no official Rust library exists). If we ship their schema
    files, we include the MIT copyright notice.
  - Maps: tracks, channels, sends and routing; nested clips with fades
    and loop points; markers; notes with per-note expressions; automation
    (tempo, time signature, mixer, plugin and built-in device parameters);
    warp points to our stretch markers; clip slots and scenes to the Clip
    Grid; VST 3 / CLAP / AU plugins with embedded state; the four generic
    built-in devices (EQ, compressor, gate, limiter) to ours.
  - **Export report:** before exporting, list what won't survive
    (comments, lesson and practice data, collaboration history, take lanes,
    Live Looper state, other built-in effects). Offer to render affected
    tracks to audio.
  - **Import report:** list anything that couldn't be mapped. Plugins
    missing on this machine keep their state and are flagged.
  - Time conversion between decimal beats/seconds and our integer
    ticks/sample frames, with defined rounding and tests.
- **Stem packages** (S): audio and MIDI stems as a simple folder or ZIP.
- **Teacher with many students** (M): a lesson library, per-student
  projects and templates.
- **Activity history** (S): who changed what, and when.

**Exit criteria**
- A full remote lesson runs inside the app with video and shared playback
  on typical home connections.
- DAWproject round-trip keeps tracks, clips, tempo and automation that the
  format supports. Tested automatically against a fixture set: our own
  projects exported and re-imported, plus **community test files** exported
  from other DAWs (e.g. shared by DAWproject users and converter projects).
  Each community file is added only once its licence or the contributor's
  permission allows redistribution in our repo, with its source and terms
  recorded alongside it.
- Every loss on export or import is listed in the report shown to the
  user. Nothing is dropped silently.

---

## Phase 5 — **Live Looper** mode

**Goal:** live looping with the immediacy of a hardware loop station, but
with every loop, layer, effect and setting **visible**: in a looper view
and on the project timeline. With native low-latency monitoring, playing
guitar through effects into loops works properly.

### 5.1 Looper engine (L)
- Any number of **loop tracks** (default 5), stereo, with level and pan in
  the mixer.
- One control per track cycles record → overdub → play → stop. The order
  is configurable (rec→dub→play or rec→play→dub).
- **Loop sync** (aligned to a master loop length) or free-length tracks.
- **Quantised actions** to beat, bar or loop end, or immediate.
- **Tempo sync**, with optional pitch-follows-tempo.
- Per-track: one-shot or loop, reverse, start mode (immediate or fade-in
  over N bars), stop mode (immediate, fade-out, or at loop end), loop
  length in bars.
- Overdub modes: **layer**, **replace** and **punch-replace**.
- **Layer history:** undo/redo per track, **bookmark** a layer and return
  to it.
- **Play mode:** multi (independent) or single (one track at a time, for
  verse/chorus switching).
- **Auto-record** when the input passes a threshold.
- **Bounce** the playing loops into one track.
- Recording length limited only by disk space.
- **Per-track speed:** half, normal or double, with the change applied
  immediately or at loop end.
- **Re-sync tolerance:** when a track restarts, snap it back into alignment
  if it's within a beat or a bar of the loop grid.
- **Measure settings** per track: auto (follow the first recording), free
  (whatever length you recorded), or a set number of bars or note values.
- **Single-play switching** happens immediately or at loop end.
- **Tap tempo**, plus a long press to clear a track.
- **External sync:** chosen tracks start and stop on incoming MIDI
  start/stop messages.

### 5.2 Performance effects (L)
- **Input effects** (before recording) and **track effects** (on playback),
  several per section. Any built-in or hosted plugin can be used.
- **Effect banks:** recallable sets, switchable live. Within a bank, allow
  one effect at a time or several.
- **Per-effect switch mode:** toggle (press on, press off) or momentary
  (on only while held).
- **Targeting:** input effects can apply to all inputs or chosen inputs.
  Track effects can apply to all loop tracks or chosen tracks.
- **Master effects** (e.g. bus compression and reverb) on chosen outputs.
- Our own performance processors: filter sweeps, beat repeat, roll, freeze,
  stutter/gate patterns, pitch bend, harmoniser, vocoder, lo-fi, radio,
  robot, tape stop, reverse delay.
- The full target palette, all our own designs:
  - **Filters:** low, high and band pass, isolator.
  - **Modulation:** phaser, flanger, chorus, tremolo, auto and manual pan,
    ring modulation.
  - **Pitch and voice:** pitch bend, octave, transpose, manual and
    automatic harmony, vocoder, formant/voice, robot, real-time pitch
    correction, guitar-to-bass.
  - **Drive and dynamics:** distortion, preamp, dynamics, sustainer,
    slow attack (volume swell).
  - **Delays:** stereo, panning, reverse, modulated, granular, tape-style.
  - **Time-mangling:** warp, twist (riser), freeze, roll, pattern
    slicer, beat repeat, beat scatter, beat shift.
  - **Colour:** lo-fi, radio, vinyl-style noise and scratch, reverb, gated
    reverb, synth (pitch-tracking synth from the input), stereo enhancer.
  - Beat-based effects work on playback only.
- **Effect step sequencer:** up to 16 steps changing an effect parameter,
  synced to the loop, with retrigger on loop start. Usable on any effect
  parameter, including plugins.

### 5.3 Rhythm (M)
- Built-in drum patterns and kits (our own content), organised by genre,
  each with several variations. Optional intro, fill and ending.
  Variation changes happen at the next bar or loop end.
- **Start triggers:** rhythm starts with the loop, when the first
  recording ends, or before the loop (count-in style: press once to start
  the rhythm, again to start recording in time).
- **Stop triggers:** never (keep running for external sync), when the loop
  stops, or when recording ends (a guide while recording).
- Time signatures including odd meters (e.g. 5/8, 7/8, 15/8).
- Any instrument or pattern track can be the rhythm source.

### 5.4 Control (M)
- MIDI controllers, footswitches and expression pedals. Press, hold and
  double-press actions. Keyboard mapping. HID devices where feasible.
- Macro controls spanning several parameters.
- **Companion remote on a phone or tablet:** builds on the Phase 2 Android
  remote app, adding large looper buttons, effect pads and XY control of
  several effects at once. For other phones and tablets, the desktop app
  also serves a simple control page over the local network (no screen
  wake-lock there, see 2.2). No internet or server of ours is needed. It
  pairs by QR code, and works on the same network only.
- **Per-loop-track output routing:** send each loop track (and the rhythm,
  and each input) to the main, alternate or headphone outputs separately.

### 5.5 Visible in DAW context (M) — *the differentiator*
- Looper view: per track, the waveform, layer stack, loop position, state
  and effects.
- Every layer is a real asset. **Capture a performance to the timeline**
  as clips for editing.
- **Phrase sets:** save and recall loop-track sets with settings.
- Collaborative: a teacher prepares a phrase set, the student performs.

### 5.6 Stage rig mode (M)
A performance setup for playing live with the sounds made in the
project, especially for guitarists and singers.
- **Patches and set lists:** each song is a patch (amp chain, effects,
  loops, rhythm), stepped through by footswitch.
- **On-screen song sheet:** lyrics, chords and notation (from the notation
  lanes or a PDF), scrolling with the song.
- Foot-controlled tap tempo, tuner on a footswitch, and MIDI foot
  controller mapping that stays in place across patches.
- Keyboard splits and layers, and chord triggers, for keyboard players.
- **Tempo sync over the local network** with other apps and devices, via
  an open LAN tempo-sync protocol. Its widely used SDK is GPLv2+ or
  proprietary. Its GPLv2-or-later option is compatible with our GPLv3
  licence (decision 0001).

**Exit criteria**
- A five-loop performance with input and track effects runs 10 minutes
  at a 128-sample buffer with no dropouts. Captured to the timeline, it
  renders identically offline.
- Loop alignment is sample-accurate (automated test).

---

## Phase 6 — Clip Grid & musical time for audio

- **Clip Grid** (L): clip slots, quantised launch, scenes, launch modes,
  follow actions, legato. Capture to timeline. Shares the Looper's
  scheduler.
- **Performance snapshots** (M): capture the state of chosen parameters,
  recall it on cue, and morph between snapshots.
- **Timeline looping during performance** (M): loop a timeline region live,
  and pre-arrange the structure of a performance before recording it.
- **Audio follows tempo** (XL): high-quality stretch in real time and
  offline, with multiple algorithms.
- **Stretch markers** (M), **transient detection and slicing** (M),
  **audio quantise** (M), **tempo detection / tempo map from a free
  performance** (M).
  - **Fit a take to the beat, full version:** detected attacks become
    stretch markers. Drag an attack onto a beat and the audio either side
    stretches to fit, with no gaps or doubled attacks, so sustained parts
    (strummed guitar, bass, vocals) work too. **Automatic quantise** to the
    grid or a groove template, with strength, a window ("only move notes
    within X ms of a beat") and a preview before applying. Add, move or
    delete markers by hand where detection gets it wrong. Grouped tracks
    share markers so they stay phase-locked.

**Exit criteria**
- Loops at foreign tempos follow tempo changes at ±20% with no obvious
  artefacts.
- A student's free-tempo recording becomes a tempo map the metronome
  follows.
- A loosely played strummed guitar take is fitted to the grid
  automatically, then two attacks are corrected by dragging, with no
  audible gaps or doubled attacks.

---

## Phase 7 — Intelligence & extensibility

- **ARA 2 hosting** (L): open SDK (Apache 2.0). Lets compatible
  pitch-editing and analysis plugins work directly on project audio.
- **Monophonic pitch editing** (XL), built in: note-level pitch, drift,
  vibrato, formant.
- **Audio-to-MIDI** (L) and **chord detection** to a **chord track** (M).
  Audio-to-MIDI feeds the Phase 3 notation lanes: draft **guitar/bass tab
  and drum staff from a recording**, with automatic string/fret assignment
  and drum-piece classification, then reviewed and corrected by the user.
- **Stem separation** (L): on-device inference, only with models whose
  weights *and* training data permit our use. Lesson use: remove the guitar
  from a backing track. Works on **any audio clip or track**, including the
  audio of imported video files, but not online video reference clips
  (platform terms forbid it, see 3.9). Each stem can go to its own track
  automatically, and keeps the source's speed and pitch settings. The most
  common open model's weights carry no stated licence, and its benchmark
  training data is for education only, so we need a licensed model, our
  own trained model, or a model the user loads.
- **Note transformations** (M): strum, arpeggiate, ornament, humanise.
- **Melodic pattern sequencer** (M).
- **Modulators** (M): LFO, step, envelope (shaper), envelope follower
  (including from a sidechain), random, sample & hold, macro knob, and
  **scripted modulators**. Able to drive any parameter, including plugin
  parameters, on any track.
  - Sync modes: project-locked, free-running, or retriggered by notes.
    Unipolar or bipolar depth.
  - **No takeover:** the parameter stays adjustable by hand while being
    modulated (modulation is applied around it).
  - Modulation connections are shown on the target's interface.
- **Real-time pitch correction** (M) as a built-in effect for vocals and
  other monophonic sources: key/scale, speed, harmonies.
- **Chord progression tool** (M): write progressions on the chord track
  quickly, and generate a matching bassline.
- **Scripting** (M): sandboxed scripts for editor actions and MIDI.
- **Controller scripting** (M) for control surfaces.
- **MIDI 2.0 / MPE** (M).

**Exit criteria**
- An ARA-capable plugin edits project audio, and its results save,
  restore and sync to collaborators (as frozen audio if they lack the
  plugin).

---

## Phase 8 — Professional scale

- Large projects: 200+ tracks, multi-core graph processing with work
  stealing, lazy loading, a built-in performance profiler.
- **Full score/notation editor** (XL), building on the Phase 3 notation
  lanes: all instruments, multi-staff scores, parts extraction and
  engraving-quality layout.
- **Articulation/expression maps** (M). **Microtonal tuning** (M).
- **Video track** (L): builds on the Phase 3 video clips with timecode,
  frame rates and frame-accurate sync for post-production.
- **Surround/immersive mixing** (XL): multichannel buses, panning, binaural
  monitoring. Format licensing reviewed before supporting any branded
  format.
- **Spectral view and repair** (L).
- **External hardware** (M): MIDI clock/MTC, external instrument/effect
  inserts with latency compensation.
- **Constrained generators** (M): rule-based rhythm/melody/chords.
- **Idea starter** (M): generate a starting loop (drums, bass, chords) by
  genre and key from our own content, then edit it freely.
- **Sound similarity search and auto-tagging** (M) in the content browser,
  with on-device analysis only. Includes "swap this kit piece for a similar
  one".
- **Automatic mastering assistant** (M): analyses the mix and proposes
  adjustable loudness, EQ and dynamics settings.
- **Audio over the local network** (M): stream audio between instances and
  devices on the same network (e.g. a teacher's and student's laptops in
  the same room) with automatic latency compensation.
- Accessibility depth (complete screen-reader workflows, keyboard access
  to every area). Themes, including high contrast. Localisation.

---

## Cross-cutting tracks (every phase)

| Track | Standing commitments |
|---|---|
| **Audio quality** | Golden-render tests, null tests, real-time safety checks, fuzzing of file decoders and of the project/bundle parsers. |
| **Performance** | Per-phase budgets (tracks/plugins at a 128-sample buffer; 60 fps UI). Regressions fail CI. |
| **Plugin compatibility** | A growing test matrix and a public compatibility list. Crash isolation is always on for scanning. |
| **Collaboration correctness** | Randomised multi-peer convergence tests. Versioned schema migrations. |
| **Security & privacy** | Signed invite codes, encrypted peer-to-peer transport, verified asset hashes, never executing content received from peers, child-safety review. |
| **Distribution** | Signed and notarised builds, reproducible release pipeline, update checks without our own server. |
| **Accessibility** | Keyboard-first, screen-reader support (toolkit-dependent, so it's a criterion in 0.1). |
| **Clean-room hygiene** | Decision records, licence checks, trademark-free naming. |
| **User research** | Every phase is validated with real musicians. Phases 1–4 always include a teacher and a student. |

---

## Dependency map

```
Phase 0 (Rust go/no-go) ─► Phase 1 (Lesson Room) ─► Phase 2 (Core audio DAW)
                                                        │
                               ┌────────────────────────┼─────────────────────┐
                               ▼                        ▼                     ▼
                         Phase 3 (MIDI &          Phase 4 (Collab       Phase 5 (Live Looper)
                          instruments)             depth)                (after 3.1 MIDI)
                               └────────────────────────┬─────────────────────┘
                                                        ▼
                                          Phase 6 (Clip Grid & stretch)
                                                        ▼
                                          Phase 7 (Intelligence & extensibility)
                                                        ▼
                                          Phase 8 (Professional scale)
```

## Open decisions

Tracked in [`docs/decisions/`](decisions/):

| # | Decision | Status |
|---|---|---|
| 0001 | Project licence: GPL-3.0-or-later | Accepted |
| 0002 | ~~Web-first with optional Electron wrapper~~ | Superseded by 0006 |
| 0003 | Implementation language: Rust, with C++/JUCE fallback | **Proceeding with Rust at risk; confirmed by spike 0.1** |
| 0004 | Collaboration: CRDT + peer-to-peer, no server we operate | Accepted 2026-10-28 (Loro) |
| 0005 | Timing model: ticks + sample frames | Accepted |
| 0006 | Native desktop application (Windows, macOS, Linux) | Accepted |
| 0007 | UI toolkit | **Open — decided by spike 0.1** |
| 0008 | Review of openDAW (exposure record) | Accepted |
| 0009 | Notation data model and tab/drum formats | Proposed |
| — | Product name and trademark search | Open — before public release |
| — | Import of a proprietary tab-editor format via file-format study (see 0009, *Deferred*) | **Parked — revisit later** |
| — | Processing audio from online video platforms (stems, re-pitch) — needs the platform's written approval (see research/VIDEO_STEMS_REPITCH.md) | **Parked — revisit later** |
| — | Browser access to ASIO and other low-latency audio devices, e.g. for a web companion (see 0002 and 0006 for why browsers can't do this today) | **Parked — revisit later** |
