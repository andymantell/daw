# 0001 — Project licence

- **Status:** Open (owner to decide in Phase 0)
- **Date:** 2026-10-24 (revised for the desktop pivot)

## Context
The project licence determines:
- Which third-party libraries we may link. Several mature time-stretch/pitch
  libraries are LGPL or GPL/commercial dual-licensed. Rust links statically
  by default, which makes LGPL use harder.
- How we use the **ASIO SDK**: offered under GPLv3 *or* a proprietary
  Steinberg licence since October 2025. VST 3 (MIT), CLAP (MIT) and ARA 2
  (Apache 2.0) are permissive, so they don't constrain the choice.
- If decision 0003 falls back to C++: **JUCE** is AGPLv3 or commercial,
  with tiers by revenue.
- The business model (paid app, free/open community project, donations).

## Options considered
1. **Proprietary** (all rights reserved). Maximum flexibility to sell. Cannot
   use GPL libraries. Needs the proprietary ASIO licence. JUCE fallback needs
   a commercial tier once revenue exceeds the free tier.
2. **GPLv3** open source. Can use GPL libraries and GPLv3 ASIO. Others may
   fork, but must share their source. Dual-licensing is possible only with
   a CLA.
3. **Permissive (MIT/Apache-2.0)**. Widest adoption. Cannot include GPL
   libraries. ASIO via the proprietary licence.
4. **Source-available / open core**: core open, some features or builds
   paid.

## Decision
_Not yet made._

## Consequences
Blocking for the stretch-library choice (spike 0.6) and for shipping ASIO
support.
