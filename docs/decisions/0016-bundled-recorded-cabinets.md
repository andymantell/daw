# 0016 — Bundled recorded cabinets and their licences

- **Status:** Accepted
- **Date:** 2026-11-06
- **Deciders:** Andy (decided 2026-11-05 that a few recorded cabinets ship,
  credited like the recorded rooms; see `docs/review-queue/owner-answers.md`)

## Context
The Speaker cabinet effect plays five cabinets made in code, or a cabinet
response file of the user's own. Andy asked for a few recorded cabinets to
come with the app as well: the one exception to "no sample files". As with
the rooms ([0011](0011-bundled-reverb-rooms.md)), each file is someone
else's recording, so we may only ship ones whose licence allows passing on,
selling and changing them inside a GPLv3 app, and we credit each as its
licence asks. Per [CLEAN_ROOM.md](../CLEAN_ROOM.md) we record each licence
as it read on the day we took the file, with its link.

## Options considered
1. Ship only cabinets under CC BY 4.0, CC BY-SA 4.0, CC0 or MIT, read at
   their source page, the same rule as the rooms.
2. Also ship free cabinet packs whose terms forbid redistribution, or allow
   it only unchanged or non-commercially. Rejected for the same reasons as
   in 0011: GPL lets anyone sell copies, and we convert every file.

## Decision
Option 1. Recorded cabinets live in `crates/effects/cabinets/`, each with an
entry in `CABINETS` in `crates/effects/src/library.rs` giving who recorded
it, the licence, the source page and what we changed. Projects name one as
`cabinet:` and its id (a room is `room:` and its id), so nothing is copied
into a project. The same test as the rooms fails if a file has no entry or
the reverse. Credits show under the cabinet when one is chosen, in its
hover text, in Help → Sound credits, and to assistants in `get_effects`.

The list shows each by the style of cabinet only, without any make or
model, as the source's own folder names do.

### What we shipped (licence read 2026-11-06)

All seven come from the "Speakers & amps" pack (`AK-SPKRS.zip`) on
Adventure Kid's [free reverb impulse responses page](https://www.adventurekid.se/akrt/free-reverb-impulse-responses/),
which describes it as "Impulse responses recorded from the cabinets of
guitar amplifiers and speakers I like" and marks it "This work is licensed
under a Creative Commons Attribution 4.0 International License." The page
also asks: "If the IR's are redistributed in any way such as a download or
in a software, please give me a credit somewhere." The page's licence
paragraph covers only the files on that page, which this pack is. The zip
holds only WAV files (no readme or licence file of its own); its SHA-256
on the day was
`d27f1a52f5dd04f5eebc2e46488f36d7ec59a0f34f7b5a95fd820ab41919e628`.

Each original is 44.1 kHz 32-bit float stereo (both channels the same).
Each was converted with `convert_room` (48 kHz, 16-bit, silence trimmed,
level set); the speaker cabinet then brings it to the same loudness as the
cabinets made here when it plays.

| Id | Listed as | File taken from `AK-SPKRS.zip` |
|---|---|---|
| `ak-vintage-us` | Vintage American | `VintageUs/AK-SPKRS_VinUs_002.wav` |
| `ak-vintage-uk` | Vintage British | `VintageUk/AK-SPKRS_VinUk_002.wav` |
| `ak-modern-us` | Modern American | `ModernUs/AK-SPKRS_ModUs_003.wav` |
| `ak-modern-uk` | Modern British | `ModernUk/AK-SPKRS_ModUk_002.wav` |
| `ak-eighties-us` | 1980s American | `80sUs/AK-SPKRS_80sUs_002.wav` |
| `ak-eighties-uk` | 1980s British | `80sUk/AK-SPKRS_80sUk_002.wav` |
| `ak-radio` | Small radio speaker | `Radio/AK-SPKRS_Radio_001.wav` |

How we chose within the pack (nothing was listened to; chosen by measuring
each file's frequency response):
- One file per named style folder, plus the radio speaker for lo-fi parts.
- The first file in most folders has 20–35 dB more low end than its middle,
  far more than a guitar cabinet, so the second (or, for the modern American
  set, the third) was taken: about 5–20 dB of low-end lift, falling away
  steeply above about 6 kHz, as a close-miked guitar cabinet does.
- The nineteen unnamed files at the top of the pack were left out: the page
  doesn't say what each one is.

### What we left out, and why
- Free cabinet packs from amp and speaker makers and from IR sellers: the
  ones we know of are free to use but not to pass on, so they cannot ship.
- No second source with an allowed licence was found on the day; more can
  be added the same way when one is.

## Consequences
- The cabinets add about 70 KB to the app.
- Adding one means: check the licence on its source page, convert it with
  `convert_room`, add a `CABINETS` entry in `library.rs`, and a row above.
- Andy still has to listen to them on a guitar
  (`docs/review-queue/2.5-amp-and-cabinet.md`).

## Sources
- Adventure Kid, free reverb impulse responses (licence and the
  "Speakers & amps" pack): https://www.adventurekid.se/akrt/free-reverb-impulse-responses/
- Creative Commons Attribution 4.0: https://creativecommons.org/licenses/by/4.0/
