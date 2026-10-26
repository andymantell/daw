# 0005 — Timing model

- **Status:** Accepted
- **Date:** 2026-10-24 (accepted 2026-10-26)
- **Deciders:** Project owner

## Context
The DAW needs musical editing (bars/beats), sample-accurate playback,
tempo changes, tempo-following audio, and loopers and clip launching
that quantise to the beat.

## Decision
- Musical positions are stored as integer **ticks at 960 PPQ**.
- Absolute positions are stored as **sample frames** at the project sample
  rate. Seconds are only used for display.
- Every clip declares its time base (musical or absolute). The tempo map
  converts between them in both directions, with exact rational arithmetic
  where possible.
- One quantised-event scheduler in the engine serves the transport,
  metronome, Live Looper and Clip Grid.

## Consequences
Integer storage avoids floating-point drift in long projects. Tempo-ramp
integration must be tested carefully.
