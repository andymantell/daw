# 0004 — Collaboration: CRDT document, peer-to-peer, no server we operate

- **Status:** Proposed (validated by spike 0.5)
- **Date:** 2026-10-24 (revised for the desktop pivot)

## Context
Teachers and students edit the same project, sometimes at the same time
and often offline. The owner has decided there will be **no backend
server**. Audio data is large; project structure is small and changes
often.

## Decision (proposed)
- **Document:** a CRDT (candidates: `yrs`, `automerge`, Loro) with a typed
  schema layer. Every merge path (live, bundle, shared folder) uses the same
  CRDT, so they all converge.
- **Assets:** immutable, content-addressed audio files inside the project
  folder.
- **Sharing modes:**
  1. *Bundle*: a single-file export that merges on import.
  2. *Shared folder*: the project in any cloud-synced directory. The app
     writes append-only update files and merges others' updates.
  3. *Live session*: the teacher hosts and the student joins by a signed
     invite code, directly peer-to-peer.
- **Live transport candidates:** WebRTC data channels (STUN hole
  punching) or QUIC with UPnP/NAT-PMP/PCP port mapping. Encrypted and
  mutually authenticated with members' keys.
- **Identity:** a per-install key pair. Roles are stored in the document
  and enforced on merge.

## Known limitation
Some network combinations (carrier-grade NAT on both sides, strict
firewalls) prevent direct connections. Without a relay, those users fall
back to bundle or shared folder.

## Open question
Should we offer an **optional relay that users run themselves** (e.g. a
teacher's own small server)? That would keep "no server we operate" while
helping hard networks. Decide by Phase 4.

## Validation (spike 0.5)
Concurrent edits, offline merge, per-user undo, document size for a
200-track project, and connection success rates across home broadband,
mobile hotspot and a strict network.
