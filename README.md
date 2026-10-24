# DAW (working title)

A **native desktop digital audio workstation** for Windows, macOS and Linux.

It starts as a tool for **guitar lessons**: teacher and student share a
project, record takes through their amp-sim plugins with low latency,
practise against backing tracks at reduced speed, and leave feedback pinned
to the music. From there it grows into a full **professional DAW** with
first-class **VST 3** support and a **Live Looper** performance mode in
which every loop and layer is visible on the timeline.

There is **no backend server**. Projects are folders on your disk, and
collaboration is peer-to-peer (the teacher hosts a session) or via project
bundles and shared folders.

> **Status:** planning. No application code yet. Phase 0 begins with a
> go/no-go spike for Rust (fallback: C++ with JUCE).

## Documents

| Document | Purpose |
|---|---|
| [docs/ROADMAP.md](docs/ROADMAP.md) | Phased plan: Rust spike, Lesson Room vertical slice, then a full DAW |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Proposed architecture: engine, plugin hosting, project format, P2P collaboration |
| [docs/CLEAN_ROOM.md](docs/CLEAN_ROOM.md) | Rules for keeping the project original and legally defensible |
| [docs/GLOSSARY.md](docs/GLOSSARY.md) | Project vocabulary (generic and original terms only) |
| [docs/research/CAPABILITY_LANDSCAPE.md](docs/research/CAPABILITY_LANDSCAPE.md) | Generic survey of DAW and loop-station capabilities, mapped to phases, plus research gaps |
| [docs/decisions/](docs/decisions/) | Decision records |

## Principles

- **Native, low-latency audio:** ASIO, CoreAudio, WASAPI, ALSA, JACK, PipeWire.
- **Plugins first-class:** VST 3 from the first release; CLAP, AU and ARA after.
- **Local-first and serverless:** your data stays on your machines.
- **Original work:** clean-room policy; no copying or reverse engineering.
- **Audio correctness:** sample-accurate timing, no dropouts, automated audio tests.
