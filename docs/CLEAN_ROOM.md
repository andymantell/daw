# Clean-Room & Independence Policy

This project is an original, standalone digital audio workstation. It is
**inspired by the general state of the art** in music software and hardware
loopers, but it is **not a clone, port, or reimplementation** of any specific
commercial product. This document sets the rules that keep it that way, and
that keep the project legally defensible from the first commit onward.

> This is an engineering policy, not legal advice. Before any commercial
> launch, fundraising, or licensing decision, have it reviewed by a qualified
> lawyer in the relevant jurisdiction.

---

## 1. What we may use as input

Permitted sources of knowledge:

- **General musical and audio-engineering knowledge**: how mixing, recording,
  MIDI, time-stretching, looping, compression, etc. work in principle.
- **Open standards, public SDKs and their published specifications**:
  MIDI 1.0/2.0, WAV/AIFF/FLAC/Ogg/Opus, VST 3 (MIT-licensed SDK since
  v3.8.0), CLAP (MIT), ARA 2 (Apache 2.0), Audio Units (Apple's public
  documentation and SDKs), ASIO (SDK under GPLv3 or proprietary licence),
  the open DAWproject exchange format, etc.
- **Academic papers and textbooks** on DSP, music information retrieval, and
  collaborative editing (CRDTs, OT).
- **Open-source code under a compatible licence**, used as a dependency with
  its licence honoured (see §5) — not copied in piecemeal.
- **Publicly observable, high-level behaviour** of products in the market
  ("DAWs generally let you loop a region"), described in our own words.
- **Our own user research**: talking to musicians, teachers and students about
  what they need.

## 2. What we must never do

- **No decompiling, disassembling, or reverse engineering** of any proprietary
  software, firmware, plugin, or file format.
- **No copying of code, assets, presets, samples, sound libraries, icons,
  skins, artwork, fonts, or documentation text** from any product.
- **No reproducing a specific product's UI layout, visual design, or "look and
  feel"** (screen arrangement, colour scheme, iconography, distinctive
  widgets). We design our own interface from first principles.
- **No use of competitors' trademarks or trademarked feature names** in the
  product, code identifiers, UI text, marketing, or repo docs. We use generic
  industry terms or our own names (see [GLOSSARY.md](GLOSSARY.md)).
- **No importing proprietary project/preset formats** unless their
  specification is openly published under terms that permit it. Open
  interchange (e.g. DAWproject, MIDI files, audio files) is the route.
- **No contributions from anyone bound by an NDA or employment terms** covering
  a competing product, where those terms would be breached by contributing.
- **No running competitor software side by side to replicate it** screen by
  screen. Comparative *user* testing ("can a student do X?") is fine.
- **No circumventing plugin copy protection** or licensing. We host plugins
  only through their published plugin interfaces.

## 3. How we write features down

- Specs in this repo describe **capabilities in generic terms** ("record
  multiple takes into stacked lanes and choose the best parts") and **our own
  design** for them.
- Specs must not name a competing product or cite one as the model to copy.
- Every significant design decision gets a short record in
  [`docs/decisions/`](decisions/) explaining *why* we chose it — this builds an
  evidence trail of independent creation.

## 4. Patents

Some audio techniques (certain time-stretching, pitch-correction, or codec
methods) have been patented in the past. Policy:

- Prefer **well-documented, published, long-established algorithms** (e.g.
  from academic literature) and **permissively licensed open implementations**.
- When adopting a novel or proprietary-sounding technique, record its source
  in a decision record.
- Do a patent review before commercial launch for any area flagged as risky
  in decision records (DSP stretching/pitch, codecs, ML separation).

## 5. Dependencies & licences

- Every third-party dependency must have a licence compatible with the
  project's own licence (still to be chosen — see
  [decisions/0001-project-licence.md](decisions/0001-project-licence.md)).
- Licences to watch closely:
  - **Copyleft (GPL/LGPL/AGPL)** libraries — e.g. some well-known
    time-stretch libraries are LGPL or GPL/commercial dual-licensed. Their
    impact depends on our licence and how they are linked. Rust links
    statically by default, so treat LGPL with particular care.
  - **ASIO SDK**: offered under GPLv3 *or* a proprietary Steinberg licence
    (since October 2025). The choice depends on the project licence.
  - **VST 3 SDK**: MIT since v3.8.0 (October 2025). The **"VST" name and
    logo are trademarks**. Check Steinberg's current trademark and logo
    usage terms before using them in the product or marketing.
  - **CLAP SDK**: MIT. **ARA 2 SDK**: Apache 2.0 (since 2021).
  - **JUCE** (fallback only, if decision 0003 picks C++): AGPLv3 or
    commercial tiers with revenue limits.
  - **ML models** (stem separation, pitch detection): both the model weights
    *and* the training data licence must permit our use.
  - **Sample/sound content**: we ship only content we create ourselves or that
    is licensed explicitly for redistribution in software.
- An automated licence check runs in CI (Phase 0 deliverable).

## 6. Contributions

- All contributors confirm their contributions are original or properly
  licensed (a DCO sign-off, `git commit -s`, is the lightweight option; a CLA
  is an alternative if a dual-licensing business model is chosen).
- Contributors must not paste code or text they do not have the right to
  contribute.

## 7. Naming

- The product name, logo and any feature names are original and checked
  against trademark registers before public launch.
- See [GLOSSARY.md](GLOSSARY.md) for the generic/original vocabulary used in
  code and UI.
