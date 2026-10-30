# 0014 — Encoders for FLAC, MP3 and Opus export

- **Status:** Accepted (built at risk; the owner can still reverse it)
- **Date:** 2026-10-30
- **Deciders:** Built while the owner was away (roadmap 3.6, Phase 3 plan item A5); the plan asked for the encoders' licences to be checked against GPLv3 first

## Context
Roadmap 3.6 asks for export to FLAC, MP3 and Ogg/Opus beside WAV, and
stems by bus. Each format needs an encoder. The app is GPL-3.0-or-later
and links statically, so each encoder must be under a GPLv3-compatible
licence ([0001](0001-project-licence.md), [CLEAN_ROOM.md](../CLEAN_ROOM.md) §5).
None of them was in the project before.

## Options considered
- **FLAC:** `flacenc` 0.5.1, pure Rust (Apache-2.0); or the reference
  C library through bindings (BSD-3-Clause), which adds a C build. The
  pure Rust one encodes a frame at a time, which suits writing as the mix
  renders.
- **MP3:** `mp3lame-encoder` 0.2.5 over `mp3lame-sys` 0.1.11, which
  builds the LAME 3.100 library from the source it carries (LGPL). There
  is no mature pure Rust MP3 encoder.
- **Opus:** `opus` 0.4.0 over `opusic-sys` 0.7.6, which builds the
  reference library (libopus) with CMake from the source it carries
  (BSD-3-Clause); or `opus-rs`, a new pure Rust encoder (0.1.x, first
  published weeks ago). The reference library is the safer sound.
- **Ogg container for Opus:** `ogg` 0.9.2, pure Rust, BSD-3-Clause.

## Decision
`flacenc`, `mp3lame-encoder`/`mp3lame-sys`, `opus`/`opusic-sys` and
`ogg`, as dependencies of the `engine` crate only.

### Licence check (read 2026-10-30)
- `flacenc`: Apache-2.0, from its crate record and the `LICENSE` file
  in its repository (https://github.com/yotarok/flacenc-rs, "Apache
  License Version 2.0, January 2004"). The Apache Software Foundation's
  page https://www.apache.org/licenses/GPL-compatibility.html says "The
  Free Software Foundation considers the Apache License, Version 2.0 to be
  a free software license, compatible with version 3 of the GPL" and that
  Apache 2 software "can therefore be included in GPLv3 projects".
- LAME 3.100: the `COPYING` file in the source the crate builds is the
  GNU Library General Public License version 2, and the library's source
  files say "either version 2 of the License, or (at your option) any
  later version". The project page https://sourceforge.net/projects/lame/
  lists GPLv2 and LGPLv2. The library can be taken under a later LGPL or
  GPL, and so combined into a GPLv3 program. Its own `LICENSE` note asks
  users to acknowledge LAME and link to its site; we do so in
  `packaging/README.txt`. The binding crates say LGPL-3.0. The LAME web
  pages at lame.sourceforge.io answered 403 to us, so the licence was read
  from the source and the project page instead.
- One LAME file (`libmp3lame/fft.c`) carries an old note about two 1980s
  algorithm patents. Patents from then have long since run out; we did
  not check further.
- libopus: BSD-3-Clause, from https://opus-codec.org/license/ ("Both the
  reference implementation and the revised implementations on
  opus-codec.org are available under the three-clause BSD license"), with
  royalty-free patent licences granted automatically to everyone. Its
  `COPYING` asks that binary distributions reproduce the notice; it is in
  `packaging/README.txt`, which ships with the Windows and Linux builds.
- `opus`: MIT or Apache-2.0; `opusic-sys` and `ogg`: BSD-3-Clause; build
  helpers `cmake` (MIT or Apache-2.0) and `autotools` (MIT). All are on
  the allow list in `deny.toml`, which CI checks.
- The FSF's licence list (https://www.gnu.org/licenses/license-list.html)
  could not be reached from the build machine (the connection was closed
  each time), so its compatibility entries for BSD-3-Clause and LGPL were
  not read for this record. Both are already on our allow list from
  [0001](0001-project-licence.md).

### Build needs
LAME is built with autotools on Linux and macOS and with the C compiler
directly on Windows; libopus needs CMake. Both are on the CI runners.
A developer on Linux or macOS needs `make` and a C compiler; on Windows,
the usual Visual Studio C++ tools and CMake.

## Consequences
- Exports can be WAV, FLAC (24-bit, lossless), MP3 (128 to 320 kbit/s,
  constant rate, with the header frame that lets players remove the
  encoder's delay) or Opus (96 to 256 kbit/s, 48 kHz).
- Builds compile two C libraries more, adding a little to clean builds.
- If we later want no C in the build, `opus-rs` (once it matures) could
  replace libopus without changing the file format.
