# 0001 — Project licence

- **Status:** Accepted
- **Date:** 2026-10-26 (options first drafted 2026-10-24)
- **Deciders:** Project owner

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

## Owner's goals
- The project is **free and open source**.
- The owner would rather others **could not fork it and make money from
  it**. No open-source licence can forbid commercial use outright, so the
  aim is to make a closed, paid fork impractical, not impossible.

## Decision
**GPLv3**, applied as `GPL-3.0-or-later` (the "or any later version"
wording recommended in the licence's own "How to Apply" section). The
licence text is in [`LICENSE`](../../LICENSE).

Why GPLv3 over the alternatives:
- Anyone may sell a fork, but must publish its complete source under the
  same licence, and anyone receiving it may pass it on free of charge.
  That makes a closed or paid-only fork impractical.
- It keeps the project open source by the standard definition. A
  non-commercial, source-available licence would forbid commercial use
  outright, but would not be open source and would rule out GPL libraries
  and the GPLv3 ASIO SDK.
- Permissive licences (option 3) and proprietary (option 1) go against the
  owner's goals.

## Consequences
- **ASIO:** use the SDK under its GPLv3 option. No proprietary ASIO
  licence is needed.
- **Libraries:** GPL-compatible dependencies are allowed, including GPL
  and LGPL stretch/pitch libraries for spike 0.6. Permissive (MIT,
  Apache-2.0, BSD) dependencies remain fine. GPLv2-*only* code is **not**
  compatible with GPLv3; GPLv2-or-later code is. The CI licence check
  enforces this.
- **Plugin SDKs:** VST 3 (MIT), CLAP (MIT) and ARA 2 (Apache 2.0) are
  compatible. Hosting third-party proprietary plugins through their
  published interfaces is not affected by our licence.
- **UI toolkit (0007):** candidates must offer a GPLv3-compatible licence.
- **C++ fallback (0003):** the framework's AGPLv3 option can be combined
  with GPLv3 code (GPLv3 section 13). The combined program would then
  carry AGPLv3's extra network-use terms, which matter little for a
  desktop app. Re-check if the fallback is taken.
- **Contributions:** DCO sign-off (`git commit -s`), no CLA. Without a CLA
  the project can't later be relicensed or dual-licensed without every
  contributor's consent. That suits the owner's goals.
- **Source files** carry an SPDX header: `SPDX-License-Identifier:
  GPL-3.0-or-later`.
- **Distribution:** every binary release must ship with the licence text
  and an offer of, or link to, the corresponding source.

This is an engineering record, not legal advice. Have it reviewed before
any public release (see [CLEAN_ROOM.md](../CLEAN_ROOM.md)).
