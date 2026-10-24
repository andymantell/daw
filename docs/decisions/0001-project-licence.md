# 0001 — Project licence

- **Status:** Open (owner to decide in Phase 0)
- **Date:** 2026-10-24

## Context
The project licence determines:
- Which third-party libraries we may bundle. Several mature time-stretch/pitch
  libraries are LGPL or GPL/commercial dual-licensed. Bundling into WASM is
  effectively static linking.
- How the optional desktop build can use the ASIO SDK: offered under
  GPLv3 *or* a proprietary Steinberg licence since October 2025. The VST 3
  SDK is MIT since v3.8.0, so it is not a constraint.
- The business model (hosted service, paid desktop app, open community).

## Options considered
1. **Proprietary** (all rights reserved). Maximum flexibility to sell. Cannot
   use GPL libraries. Needs the proprietary ASIO licence.
2. **AGPLv3 / GPLv3** open source. Can use GPL libraries and GPLv3 ASIO.
   Hosted forks must share changes (AGPL). Dual-licensing is possible only
   with a CLA.
3. **Permissive (MIT/Apache-2.0)**. Widest adoption. Cannot bundle GPL
   libraries. ASIO via the proprietary licence.
4. **Open core**: permissive or AGPL core with proprietary hosted features.

## Decision
_Not yet made._

## Consequences
Blocking for spike 0.6 (stretch library choice) and Phase 8 ASIO.
