# DAW (working title)

A **web-first, collaborative digital audio workstation**.

It starts as a tool for **guitar lessons**: a teacher and student share a
project, record takes, practise against backing tracks at reduced speed, and
leave feedback pinned to the music. From there it grows into a full
**professional DAW**, including a **Live Looper** performance mode where
every loop and layer is visible on the timeline.

> **Status:** planning. No application code yet. Start with the roadmap.

## Documents

| Document | Purpose |
|---|---|
| [docs/ROADMAP.md](docs/ROADMAP.md) | Phased plan from foundations, to the Lesson Room vertical slice, to a full DAW |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Proposed technical architecture (engine, document model, sync, desktop wrapper) |
| [docs/CLEAN_ROOM.md](docs/CLEAN_ROOM.md) | Rules for keeping the project original and legally defensible |
| [docs/GLOSSARY.md](docs/GLOSSARY.md) | Project vocabulary (generic and original terms only) |
| [docs/research/CAPABILITY_LANDSCAPE.md](docs/research/CAPABILITY_LANDSCAPE.md) | Generic survey of DAW and loop-station capabilities, mapped to phases |
| [docs/decisions/](docs/decisions/) | Decision records |

## Principles

- **Web first.** Runs in a modern browser. An optional Electron wrapper adds
  native low-latency audio (ASIO, CoreAudio, JACK) and native plugins later.
- **Collaborative and local-first.** Shared projects sync in real time and
  work offline.
- **Original work.** Clean-room policy; no copying or reverse engineering of
  other products.
- **Audio correctness.** Sample-accurate timing, no dropouts, automated
  audio tests.
