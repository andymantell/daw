# 0006 — Native desktop application

- **Status:** Accepted
- **Date:** 2026-10-24
- **Deciders:** Project owner
- **Supersedes:** [0002](0002-web-first-optional-desktop.md)

## Context
The web-first plan hit a growing list of limitations:
- No ASIO access; shared-mode output on Windows; no control over buffer
  size; fixed processing blocks.
- **No VST plugins** (browsers cannot load native code). VST support is a
  must.
- Serverless collaboration was not achievable in a browser either.
- Storage eviction, no general disk access outside Chromium, memory
  ceilings, browser-reserved shortcuts, uneven MIDI/device APIs (no Web MIDI
  in Safari), and audio work limited to one thread per audio context.

## Decision
Build a **native desktop application** for Windows, macOS and Linux. There
is no web version.

## Consequences
- Low-latency monitoring through software effects becomes a core
  workflow. VST 3 hosting arrives in the first vertical slice.
- New responsibilities: code signing and notarisation, installers and
  updates, plugin compatibility testing, Apple Silicon/Intel plugin
  handling, OS microphone permissions, Linux packaging.
- Collaboration is peer-to-peer and file-based (decision 0004).
