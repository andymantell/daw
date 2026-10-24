# 0004 — Collaboration model: CRDT document + content-addressed assets

- **Status:** Proposed (confirm in spike 0.5)
- **Date:** 2026-10-24

## Context
Teachers and students edit the same project, sometimes at the same moment
and sometimes offline. Audio data is large; project structure is small and
changes often.

## Options considered
1. **CRDT (Yjs)**: mature, offline-first, awareness channel for presence,
   many sync providers; MIT licence.
2. **CRDT (Automerge)**: strong history model; different performance profile.
3. **Server-authoritative operational transform**: needs a central server
   for every edit; weaker offline story.
4. **Lock-based editing**: simple, poor collaboration experience.

## Proposed decision
Yjs for the project document, with a typed schema layer on top. Audio
assets are immutable, content-hashed files in object storage, referenced
from the document.

## Validation (spike 0.5)
Concurrent edits from two clients, offline merge, per-user undo, document
size and load time for a 200-track synthetic project.
