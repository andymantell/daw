# 0009 — Notation data model and tab/drum formats

- **Status:** Proposed (implementation in Roadmap Phase 3.8)
- **Date:** 2026-10-24

## Context
Guitar/bass tab and drum notation are first-class timeline items, used to
recreate songs. They need a data model that stays in sync with playback and
MIDI, and import/export formats people actually have.

## Decision (proposed)
- **Model:** notation is a view of a track's note data. Notes carry optional
  performance attributes (string, fret, techniques, drum articulation). The
  track holds a tuning (tab) or drum map (drums). No separate notation
  document.
- **Supported formats:**
  - **MusicXML 4.0**: open, published as a W3C Community Group Final Report
    under the W3C Community Final Specification Agreement. Primary format for
    tab and percussion import/export.
  - **Standard MIDI files**: drum parts use the General MIDI percussion key
    map by default. Tab positions are assigned automatically.
  - **Plain-text (ASCII) tab**: no formal standard. Best-effort parsing with
    a user review step.
  - **Print/PDF** export.
- **Rendering:** our own layout engine, using SMuFL-compliant fonts. The
  reference SMuFL font is under the SIL Open Font License: it may be bundled
  and redistributed (including commercially), but not sold on its own, and
  derivatives must stay under the OFL and not use the reserved font name.
- **Drum notation:** default staff positions and noteheads follow widely
  published drumset-notation conventions. Because kits vary, the drum map is
  always editable and printed as a legend.

## Excluded: proprietary tab-editor formats
The most popular tablature editor's file formats have **no official
published specification**. Public documentation of them comes from
community reverse engineering, and open-source parsers exist. Under
[CLEAN_ROOM.md](../CLEAN_ROOM.md) §2 we don't import proprietary formats
unless their specification is openly published, and the project owner has
ruled out reverse engineering. So:
- No import of those formats, and no use of third-party parsers for them.
- Users who have such files can export MusicXML or MIDI from that editor
  and import the result.
- Revisit only if an official specification is published under terms that
  allow implementation.

## Deferred: interoperability import via file-format study (revisit later)
Noted on 2026-10-24 as a possible future exception. **Nothing to do now.**
- **Legal background (not legal advice):** in the EU, data file formats
  are not protected by copyright, and lawful users may observe, study and
  test a program's functioning (CJEU, *SAS Institute v World Programming*,
  2012). UK law has equivalent provisions (CDPA ss.50B, 50BA; contract
  terms overriding them are void under s.296A). US law differs, and licence
  terms banning reverse engineering have been enforced there. Courts have
  ruled against reverse engineering that goes beyond the exceptions
  (*IBM v LzLabs*).
- **Only acceptable method:** study data files we create ourselves, with
  known content, to document the structure. No decompiling of the editor,
  no copying of third-party parser code, no copying of documentation text.
- **Process if adopted:** one person writes a format description from
  test files alone. Someone who hasn't seen existing parsers implements
  it. Decision records at each step. Legal review per distribution
  country before release.
- **Scope if adopted:** import only, newest (XML-based) format first,
  older binary formats only if users need them. MusicXML remains the
  export format. Factual compatibility wording only, with no logos or
  implied endorsement.
- **Before adopting:** amend [CLEAN_ROOM.md](../CLEAN_ROOM.md) to allow
  file-format study for interoperability, keeping the bans on decompiling
  and on copying third-party code.

## Consequences
- MusicXML quality is critical. Round-trip tests cover tab techniques and
  percussion.
- Automatic fret assignment is required for MIDI and audio-sourced notes.

## Sources
W3C Music Notation Community Group (MusicXML 4.0, June 2021); SMuFL site and
font licensing; Percussive Arts Society drumset-notation guide (Weinberg,
2002) as a general reference for conventions; community format notes
(consulted only to confirm the absence of an official specification).
