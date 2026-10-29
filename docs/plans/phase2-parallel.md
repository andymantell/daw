# Phase 2: what to build next, and in what order (2026-10-29)

Read from [ROADMAP.md](../ROADMAP.md) Phase 2 on main at f89e828. Phase 1 is
built except live sessions, which wait on the owner's network test.

Each item below is one feature thread with its own branch and PR. Items in
the same set don't share the files that carry their real work, so they can
run side by side. `app/main.rs` (menus, settings, the transport bar) is
touched lightly by nearly every item. That is handled by rebasing, and by
each item keeping its interface in its own module with only a small hook in
`main.rs`.

## Set A: start now

| # | Item (plain English) | Roadmap | Main code areas | Needs from the owner |
|---|---|---|---|---|
| A1 | **Built-in effects**: EQ, compressor, gate, limiter, delay and reverb (algorithmic and convolution), offered beside plugins in each track's effects list and saved in the project. They run through the same insert interface as plugins. | 2.5 | new `effects` crate, `app/fx.rs`, `project/model.rs` (a built-in effect kind) | Listening to each effect; nothing blocks building. |
| A2 | **Phone remote, desktop side and Android app**: a control protocol over the local network, QR pairing with a one-time secret, and an Android app with transport, record, arm, count-in, metronome, undo-last-take, level meters and a timeline overview. The desktop stays in charge, and a lost connection never affects a recording. Starts with a short spike on the app's language and toolkit, and whether it can share Rust code. Punch ranges are added once A3 lands. | 2.2 | new `remote` crate, new `android/` folder, a CI job building the app package, `app/main.rs` (Settings → Remote) | An Android phone to install a test build on (sideloaded, unsigned). Later: store listing or download from our releases. |
| A3 | **Recording depth, part 1**: punch in and out (by hand or over a range), pre-roll and post-roll, recording many armed tracks from many inputs at once, and retrospective capture (keep the last few minutes of input even when not recording). | 2.2 | `engine/record.rs`, `engine/engine.rs` (commands), the transport bar | A multi-input test on the owner's audio interface. |
| A4 | **Meters**: peak and RMS, loudness (by the published broadcast loudness standard), true peak, a spectrum and phase correlation, on the master and each track. Tested against the standard's reference signals. | 2.3 | new `engine/meter.rs`, new `app/meters.rs`, a tap on the master output in `engine/stream.rs` | None. |
| A5 | **Plugins in their own process while playing**: a choice per plugin of in the app, a shared host process or its own process, so a crashing plugin no longer takes the app down (the known limit in 1.3). Plus plugin presets in our own format and the plugin's own presets. | 2.4 | `plugins` crate (a host process and an insert that talks to it), `app/fx.rs` (a small menu) | Trying it with real plugins on Windows. |
| A6 | **Tempo and time signature changes**: a tempo map with changes and ramps, and time signature changes, followed by the metronome, count-in, bar numbers, loop and markers. | 2.7 | `engine/time.rs`, `engine/metronome.rs`, `project/model.rs`, the timeline ruler in `app/timeline.rs` | None. |
| A7 | **Searchable command list and your own key commands**: every action is a named command, found by typing, with key commands the user can change. Saved layouts (B2) get their keys from this. | 2.1 | new `app/commands.rs`, the key handling and shortcuts window in `app/main.rs`, `app/a11y.rs` | Trying the default keys. |
| A8 | **Project templates and snapshot differences**: start a project from a template ("Guitar lesson", "Band demo"), save your own, and see what changed between two snapshots. | 2.8 | `app/project.rs` (new project), `app/snapshots.rs`, `project/snapshots.rs` | Which templates to include. |
| A9 | **Offline processing**: normalise, reverse and change gain of a clip, making a new audio file so the original is kept and undo works. | 2.1 | new `engine/process.rs`, the clip menu in `app/timeline.rs` (small) | None. |

A1 and A5 both touch `app/fx.rs`, only lightly. A6 owns the timeline ruler,
and A9 adds only a clip-menu entry. A2 adds punch ranges after A3 has merged.

## Set B: start as the items they build on merge

| # | Item | Roadmap | Starts after | Main code areas |
|---|---|---|---|---|
| B1 | **Fades, crossfades and clip gain, plus grid, snap, zoom, nudge, slip and ripple editing**: curve shapes, gain envelopes, and the editing tools built on the tempo map. | 2.1 | A6 | `app/timeline.rs`, clip playback in `engine/graph.rs`, `project/model.rs` (clip fields) |
| B2 | **Panes and detachable windows**: dock, split, tab and collapse panes in one window, tear any pane off onto another screen, saved layouts with key commands, safe when a screen goes missing. Runs while no other item restructures the main window. | 2.9 | A7 | `app/main.rs` (layout), new `app/workspace.rs`, local settings |
| B3 | **Mixer routing**: sends (before or after the fader), effect return and group buses, folder tracks, control-group faders, sidechain inputs, and delay compensation across the whole engine, checked by automated null tests. The mixer moves into its own module and pane. | 2.3 | A1, A5 (both settle the insert interface); best after B2 | `engine/graph.rs`, `project/model.rs`, new `app/mixer.rs` |
| B4 | **Amp and cabinet simulation, and a player for neural amp captures**: our own amp models and impulse responses, and loading open-format captures the user supplies. The inference library's licence is checked first. | 2.5 | A1 | `effects` crate (its own module) |

## Set C: later

- **Take lanes and comping** (2.2): loop recording into stacked lanes, swipe
  to comp with automatic crossfades, several comps per set of takes. After
  A3 and B1.
- **Fix a badly timed take, first cut** (2.1): attack detection, slice and
  snap to the grid with strength, across grouped tracks. After B1, and
  group editing.
- **Group editing** (2.1): edit several tracks together, e.g. drum mics.
  After B1.
- **Automation** (2.6): lanes for every parameter, with read, touch, latch
  and write. After B1 and B3.
- **Monitor section** (2.3): headphone mixes per performer, talkback, dim
  and mono, speaker switching. After B3. Needs the owner's interface outputs.
- **Plugin depth, the rest** (2.4): multi-output plugins, sidechains into
  plugins, live delay changes, editor scaling, mapping old plugin versions
  to new ones, a compatibility list. After A5 and B3.

## Still waiting on the owner

Unchanged from Phase 1: live sessions and peer-to-peer transfer (network
test), real deletion out of history and keys per project (privacy
decisions), signed installers (0.7), and the speed and pitch library choice
(0.6 listening). Each Phase 2 thread adds its own review steps as a new file
in [review-queue/](../review-queue/).
