# 0002 — Web-first with an optional Electron desktop wrapper

- **Status:** Accepted
- **Date:** 2026-10-24
- **Deciders:** Project owner

## Context
Collaboration and zero-install access are key product goals, especially for
guitar lessons. Browsers cannot reach ASIO drivers. On Windows, they output
through shared-mode WASAPI, which limits low-latency software monitoring.

## Decision
- The product is a web application first. Every core feature must work in
  supported browsers.
- An **optional Electron wrapper** (Roadmap Phase 8) loads the same web build
  and adds a native audio backend (ASIO/CoreAudio/JACK/PipeWire) and native
  plugin hosting.
- The engine interface (Architecture §5) is designed from day one so a
  native backend can be added without changing UI or document code.

## Consequences
- Recording relies on calibrated latency compensation plus direct/hardware
  monitoring in the browser.
- Live Looper mode works in the browser with direct monitoring. Monitoring
  guitar through software effects at low latency is a desktop-wrapper
  feature.
- Desktop-only features (native plugins) need a fallback (frozen audio) so
  web collaborators can still play the project.
