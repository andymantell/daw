# 0003 — Implementation language: Rust, with C++/JUCE as fallback

- **Status:** Proposed. Decided by spike 0.1 (Roadmap Phase 0)
- **Date:** 2026-10-24

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

## Sources
Rust ecosystem survey (October 2026): docs for `vst3`, `vst3-host`,
`cpal` 0.18, `clack`, and baseview-based UI integrations. JUCE 8 licence
announcements on the JUCE forum.
