# Phase 0 spikes

Throwaway prototypes for the Rust go/no-go spike
([ROADMAP](../docs/ROADMAP.md) 0.1, [decision 0003](../docs/decisions/0003-implementation-language.md)).
Nothing here ships. Licence: GPL-3.0-or-later.

| Test | Crate | Status |
|---|---|---|
| **A. Audio I/O** (ASIO on Windows) | — | Not started. Needs the owner's PC and interface. |
| **B. VST 3 hosting** | `vst3-scan` (step one: list installed plugins) | Scanner built. Loading plugins not started. |
| **C. UI** | — | Not started. |
| **D. Engine** | `engine` | Passing (see below). |

Test machine: Windows only for now, so the macOS (CoreAudio) half of A
and B is still open.

## D. Engine

`engine` has a lock-free command queue from the UI thread to the audio
thread, and swaps the whole render graph by moving a pointer through that
queue. The audio thread crossfades the old and new graphs over one block,
then hands the old graph back through a second queue so it is freed on the
UI thread.

`cargo test --release -p spike-engine` checks:

- **No allocation on the audio thread.** A global allocator counts every
  allocate, reallocate or free made while a thread is marked real-time.
  Over 30 minutes of audio (675,000 blocks of 128 samples at 48 kHz,
  rendered faster than real time), with the UI thread sending (in one run) about 11
  million gain changes and 1.6 million graph swaps, the count is zero.
- **Glitch-free swaps.** The output never steps further between samples
  than its oscillators naturally can. A control test shows that a hard swap
  *does* trip the same detector, and the crossfade removes it.
- **The detector works.** A deliberate allocation inside the real-time
  section is counted.

Limits: this runs without a sound card and faster than real time. Timing
under a real driver callback is part of test A.

## B, step one: `vst3-scan`

Lists every `.vst3` in the standard folders (on Windows,
`C:\Program Files\Common Files\VST3`) plus any folders passed as
arguments, and reads each bundle's `moduleinfo.json` for name, vendor,
version and category. It only reads files and never loads plugin code.
The report is saved as `vst3-scan-report.txt` in the current folder.

Download it from the latest **spikes** workflow run on GitHub (artifact
`spikes-windows-x64`).
