# 0003 — DSP language: Rust compiled to WASM

- **Status:** Proposed (confirm in spike 0.3)
- **Date:** 2026-10-24

## Context
The audio thread needs predictable performance with no garbage-collection
pauses. The same DSP code should run in the browser (WASM in an
AudioWorklet) and natively in the optional desktop build.

## Options considered
1. **Rust → WASM**: memory-safe, no GC, good WASM tooling, compiles natively
   for desktop.
2. **C++ → WASM (Emscripten)**: mature DSP ecosystem; less memory safety.
3. **TypeScript in the worklet**: simplest; GC pauses and slower DSP are a
   risk at scale.
4. **AssemblyScript**: TS-like syntax compiled to WASM; smaller ecosystem.

## Proposed decision
Rust for the DSP core. Thin TypeScript glue in the worklet.

## Validation (spike 0.3)
Measure track/effect counts before dropouts against a TypeScript baseline,
build ergonomics, bundle size and debugging experience.
