# 0010 — Plugins in a helper process

- **Status:** Accepted (first cut built 2026-10-29; owner checks pending)
- **Date:** 2026-10-29
- **Deciders:** project owner, by way of roadmap 2.4

## Context
Phase 1 ran every plugin inside the app, so one crashing plugin took the
app down with it (noted when building frozen audio, 1.3). Roadmap 2.4
asks for a choice per plugin: in the app, a shared host process, or a
process of its own. The architecture sketch (§5) pictured shared-memory
audio buffers and lock-free queues between the engine and the host
process.

Constraints: the audio thread must never block on another process; the
app must notice a crash or a hang and carry on; nothing may be added
that the licence check (`deny.toml`) refuses; the first cut should be
testable in CI on Linux, macOS and Windows without a real plugin.

## Options considered
1. **Shared memory and OS synchronisation.** Lowest cost per block, but
   needs platform code for mapping and signalling on each OS, a new
   dependency or two, and is hard to test without the owner's PC.
2. **Pipes with a one-block pipeline.** The helper is the app itself
   run with `--host-plugins`, talking over its standard input and
   output in small length-prefixed messages. A live insert queues each
   block for an I/O thread and plays the answer to the block before, so
   the audio thread never waits; the extra block is reported as latency
   and the engine makes up for it. No new dependencies; the same code on
   every OS.
3. **Keep everything in-process** and rely on crash recovery (1.8).
   Loses the live session and the take in progress.

## Decision
Option 2 for the first cut, behind an interface (`Plugin` and
`Processor` enums over local and remote kinds) that option 1 can slot
into later without changing the app. The cost: one audio block of extra
delay for plugins in a helper, shown in the effects window, and a copy
of each block down and back up a pipe. A watchdog stops a helper that
has not answered for 10 s. The default for a plugin is the shared
helper; the user picks per plugin in Settings → Plugins.

Presets use the VST 3 preset file layout for both the user's own and
the plugin's own, so they travel between hosts.

## Consequences
- Crashes and hangs in plugins become a "Stopped" line on the effect
  with a Restart button, and the app plays on.
- Plugins in a helper add a block of latency to live monitoring; a
  guitarist who notices it can set that plugin to run in the app.
- Plugin editors opened for a helper plugin live in the helper's
  process; the helper pumps their window messages. Untested until the
  owner tries it on Windows.
- Offline rendering (export, frozen audio) uses a waiting variant that
  adds no delay.
- Follow-up if measurements ask for it: shared-memory buffers (option
  1) behind the same interface; a cap on open editors; editor scaling.

## Sources
- VST 3 interface definitions, as the `vst3` Rust bindings expose them.
- Our own tests in `crates/plugins/tests/remote.rs` (crash, hang,
  latency and alignment under paced blocks).
