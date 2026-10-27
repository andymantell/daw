# 0003 — Implementation language: Rust, with C++/JUCE as fallback

- **Status:** Proceeding with Rust at risk (2026-10-27). Final go/no-go still decided by spike 0.1
- **Date:** 2026-10-24 (at-risk update 2026-10-27)

## Context
A native DAW needs predictable real-time performance, solid VST 3 hosting
including embedded plugin editor windows, low-latency I/O on every
operating system, and a UI that stays smooth with large projects.

## Options considered
1. **Rust.** Memory safety and data-race freedom in the engine, and easy
   cross-platform builds with Cargo. Permissive candidate libraries:
   `cpal` (supports ASIO), `vst3` bindings (MIT/Apache, pre-generated since
   0.3), `clack` (CLAP). Risks: plugin hosting and editor embedding are
   largely ours to build; UI toolkits are less proven for large DAW UIs;
   no ready-made AU hosting; smaller audio community.
2. **C++ with JUCE.** Mature VST 3/AU hosting and plugin windows, audio I/O
   including ASIO, and a full UI framework. Risks: AGPLv3 or commercial
   licence; we'd be tied to the framework's design; no official CLAP hosting
   found; C++ memory-safety burden.
3. **C++ on our own stack of small libraries.** Full ownership, with the same
   hosting/UI work as Rust but without Rust's safety benefits.

## Proposed decision
Rust, **conditional on spike 0.1** passing:
- **A. Audio I/O:** ASIO + CoreAudio full duplex at 64–128 samples, no
  dropouts for 30 minutes.
- **B. VST 3 hosting:** 10–15 popular plugins process, report latency,
  automate, save/restore state, and show embedded editor windows on
  Windows and macOS.
- **C. UI:** 300+ clip timeline at 60 fps, text input, menus,
  drag-and-drop, basic screen reader support.
- **D. Engine:** lock-free command path and glitch-free graph swap, with
  no audio-thread allocation.

**Fallback rule:** if B or C proves disproportionately hard, switch to
C++/JUCE before product code exists, and re-run B and C to confirm.

## At-risk update (2026-10-27)
The owner chose to continue building in Rust before the on-device parts of
spike 0.1 have run, because the only test machine is unavailable for a
week. Status of the pass criteria:

| Test | Status |
|---|---|
| A. Audio I/O | Not run. Needs the owner's Windows PC and interface. macOS untested (no Mac available). |
| B. VST 3 hosting | Not run. A read-only plugin folder scanner is built. |
| C. UI | Not run. |
| D. Engine | **Passed** in CI on Windows and Linux (see `spikes/README.md`). |

To limit the cost if B or C later fails and the fallback rule applies:
- Build platform-independent parts first (project document and schema,
  collaboration, timing maths, DSP, offline rendering). Their designs carry
  over to C++ even if the code doesn't.
- Hold back on UI and plugin-window code until tests B and C have run.
- Revisit this record as soon as A–C have results.

## Sources
Rust ecosystem survey (October 2026): docs for `vst3`, `vst3-host`,
`cpal` 0.18, `clack`, and baseview-based UI integrations. JUCE 8 licence
announcements on the JUCE forum.
