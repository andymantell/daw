# 0008 — Review of openDAW (exposure record)

- **Status:** Accepted (record of exposure)
- **Date:** 2026-10-24

## Context
To compare approaches, we reviewed openDAW (github.com/andremichelle/openDAW),
an open-source browser DAW licensed under AGPLv3 (or commercial). Under
[CLEAN_ROOM.md](../CLEAN_ROOM.md) §2, we don't use the source code of
copyleft DAW projects as implementation reference, and we record any
exposure.

## What was read
- The README, the repository overview (`introduction.md`) and its desktop
  plan (`plans/tauri.md`).
- Grep-level excerpts to answer high-level questions: which servers it
  connects to, whether it needs cross-origin isolation, whether it asks for
  persistent storage, its MIDI support message, and that recording
  placement combines output and input latency.

## What was taken
Only high-level, non-copyrightable observations, used to confirm our own
direction:
- Even a mature web DAW runs its own servers for live collaboration and
  relays, and has no VST support.
- Its own desktop plan moves to native audio I/O and native plugin hosting.
- Neural amp model support is valuable for guitarists. This is a
  user-facing feature idea, specified independently in ROADMAP 2.5.

No code, schemas, data structures, file formats or internal designs were
copied or will be used. Our implementations come from our own design,
published specifications and the literature.
