# 0012 — Amp capture player and its inference library

- **Status:** Accepted (built at risk; the owner can still reverse it)
- **Date:** 2026-10-30
- **Deciders:** Built while the owner was away (roadmap 2.5, plan item B4); the roadmap already asked for an open-format capture player on a licence-compatible library

## Context
Roadmap 2.5 asks for a player for neural amp and pedal captures in the
open `.nam` file format, so a teacher or student can use the sound of a
real amp they (or someone else) captured, without a plugin. The roadmap
asked for the inference library's licence to be checked before we
depend on it. The app is GPL-3.0-or-later and links statically, so the
library must be under a GPLv3-compatible licence ([0001](0001-project-licence.md),
[CLEAN_ROOM.md](../CLEAN_ROOM.md) §5). The clean-room policy also says
open code is used as a dependency with its licence honoured, not copied
in piecemeal.

## Options considered
1. **`nam-rs` 0.4.0** (crates.io), pure Rust, real-time safe (no
   allocation in the audio call), reads the file and runs WaveNet, LSTM
   and the newer "slimmable" models. MIT. Its own dependencies are
   `serde`, `serde_json` and `thiserror` (MIT or Apache-2.0), all already
   used elsewhere in the workspace.
2. **The reference C++ inference library** (`NeuralAmpModelerCore`), MIT,
   with a C++ toolchain and the Eigen library (MPL-2.0) in our build.
   Workable, but adds a C++ build to every platform for no gain.
3. **`NeuralAmpModeler-rs` 1.0.1**, Apache-2.0 (also GPLv3-compatible),
   but needs Rust 1.99; our workspace is on 1.88, and it has far fewer
   users so far.
4. **Write our own inference.** The weight layout is defined only by the
   reference code, so this would mean translating it, which the
   clean-room policy steers away from.

## Decision
Option 1, `nam-rs` 0.4.0, as a dependency of the `effects` crate only.

### Licence check (read 2026-10-29 and 2026-10-30)
- `nam-rs`: MIT. Read from its `LICENSE` file in its repository
  (https://github.com/OpenSauce/nam-rs, "MIT License, Copyright (c) 2026
  Leigh"), and the crates.io record for every published version says MIT.
  Its README and `NOTICE` say it is a port of the three projects below,
  whose licence texts it reproduces.
- `neural-amp-modeler` (the trainer that defines the file format): MIT,
  read from https://github.com/sdatkinson/neural-amp-modeler `LICENSE`
  ("Copyright (c) 2019-2025 Steven Atkinson").
- `NeuralAmpModelerCore` (the reference C++ inference): MIT, read from
  https://github.com/sdatkinson/NeuralAmpModelerCore `LICENSE`
  ("Copyright (c) 2023 Steven Atkinson").
- `NeuralAudio` (a C++ runtime it was ported from): MIT, read from
  https://github.com/mikeoliphant/NeuralAudio `LICENSE` ("Copyright (c)
  2024 Mike Oliphant").

MIT is compatible with GPLv3: MIT code may be combined into a GPLv3
program as long as its copyright and permission notice travel with it.
The CI licence check (`cargo deny`) covers the crate and its
dependencies.

### Capture files
No capture files come with the app. Captures are made by people from
their own gear and carry whatever terms their maker chose, so the user
supplies their own, as the roadmap says. We would bundle only captures
we make ourselves or that are licensed for redistribution, each recorded
here first.

### What we built around it (our own design)
- A sample-rate converter (windowed-sinc band-limited interpolation, from
  J. O. Smith's published description) so a capture made at 48 kHz plays
  correctly in a 44.1 kHz project, with its delay reported to the track.
- Input and output levels, an optional "even out loudness" using the
  level a capture file records, and a DC cut after the network, which
  the library leaves to the host.
- Saving: the capture file is copied into the project as an asset, like
  a reverb's response file, so a project opens with the same sound on
  another computer.

## Consequences
- The MIT notices of `nam-rs` and the projects it ports must ship with
  the app. The app does not yet ship a third-party notices file for any
  of its MIT dependencies; that is a general release task, not specific
  to this one.
- Captures that use features `nam-rs` doesn't run (multi-channel input
  and a few rare options) are refused with its message when chosen.
- The product names the format by its file extension only ("capture file
  (.nam)"); no third-party names appear in the interface.

## Sources
- nam-rs README and crate docs: https://github.com/OpenSauce/nam-rs
- J. O. Smith, "Digital Audio Resampling Home Page", CCRMA, Stanford:
  https://ccrma.stanford.edu/~jos/resample/
