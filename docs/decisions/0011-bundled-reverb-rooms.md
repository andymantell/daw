# 0011 — Bundled reverb rooms and their licences

- **Status:** Accepted
- **Date:** 2026-10-29
- **Deciders:** Andy (asked for credited rooms bundled with the app, credit shown when a room loads)

## Context
The Room reverb ships with a small library of recorded rooms. They are
installed with the app, not stored in projects; a project only names the
room it uses. Each file is someone else's recording, so we may only ship
ones whose licence allows redistribution and changes inside a GPLv3 app,
and we must credit them as their licence asks. Per [CLEAN_ROOM.md](../CLEAN_ROOM.md)
we also record each licence as it read on the day we took the file, with
its link.

## Options considered
1. Ship only rooms under CC BY 4.0, CC BY-SA 4.0, CC0 or MIT, read at the
   source page of each one.
2. Also ship collections whose terms are non-commercial, no-derivatives,
   "unaltered copies only", or unstated. Rejected: GPL lets anyone sell
   copies of the app, and we convert, trim and decode every file.

## Decision
Option 1. Every room in `crates/effects/rooms/` has an entry in
`crates/effects/src/library.rs` naming who made it, the licence, the source
page and what we changed. A test fails if a file has no entry or the reverse.

### What we shipped (licences read 2026-10-29)

All OpenAIR spaces below come from the OpenAIR library (Audiolab, University
of York). Each space's own page said: "This work is licensed under a Creative
Commons Attribution 4.0 International License." The credit is the
page's "Attribute this work to" text, or its measurement team where that is
blank. The files were downloaded from the University of York's file server for
OpenAIR (`https://webfiles.york.ac.uk/OPENAIR/IRs/<space>/`, "File taken"
below), because the OpenAIR website's own certificate had expired on the
day.

| Room | Place and source page | Credited to | File taken |
|---|---|---|---|
| `concert-hall-jack-lyons` | [Jack Lyons Concert Hall, University of York](https://www.openair.hosted.york.ac.uk/?p=571) | Audiolab, University of York: Alex Duffell, Aishwarya Sridhar and Zhong Li (OpenAIR) | `b-format/rir_jack_lyons_lp1_96k.wav` |
| `cathedral-york-minster` | [York Minster](https://www.openair.hosted.york.ac.uk/?page_id=797) | Audiolab, University of York: Damian T. Murphy (OpenAIR) | `stereo/minster1_000_ortf_48k.wav` |
| `chapel-st-albans` | [Lady Chapel, St Albans Cathedral](https://www.openair.hosted.york.ac.uk/?page_id=595) | Audiolab, University of York: Marcin Gorzel, Gavin Kearney, Aglaia Foteinou, Sorrel Hoare and Simon Shelley (OpenAIR) | `stereo/stalbans_a_ortf.wav` |
| `church-patrington` | [St Patrick's Church, Patrington](https://www.openair.hosted.york.ac.uk/?page_id=722) | Aglaia Foteinou and Damian T. Murphy (OpenAIR) | `stereo/ortf_s1r1.wav` |
| `theatre-dixon-studio` | [The Dixon Studio Theatre, University of York](https://www.openair.hosted.york.ac.uk/?page_id=452) | Ben Lavin, Darren Robinson and Ya-Hsin Chou, University of York (OpenAIR) | `b-format/r1_rir_bformat.wav` |
| `stairwell-york` | [Stairway, University of York](https://www.openair.hosted.york.ac.uk/?page_id=678) | Audiolab, University of York: Simon Shelley (OpenAIR) | `stereo/stairwell_ortf.wav` |
| `typing-room-terrys` | [Terry's Typing Room, York](https://www.openair.hosted.york.ac.uk/?page_id=740) | Audiolab, University of York: Damian T. Murphy (OpenAIR) | `stereo/terrys_typing_ortf.wav` |
| `marble-hall-elveden` | [Marble Hall, Elveden Hall, Suffolk](https://www.openair.hosted.york.ac.uk/?page_id=459) | Matt Rogalsky (OpenAIR) | `stereo/1a_marble_hall.wav` |
| `warehouse-terrys` | [Terry's Factory Warehouse, York](https://www.openair.hosted.york.ac.uk/?page_id=735) | Audiolab, University of York: Damian T. Murphy (OpenAIR) | `stereo/terrys_warehouse_stereo.wav` |
| `reactor-hall-r1` | [R1 Nuclear Reactor Hall](https://www.openair.hosted.york.ac.uk/?page_id=626) | Audiolab, University of York (OpenAIR) | `stereo/r1_ortf-48k.wav` |
| `tunnel-innocent-railway` | [Innocent Railway Tunnel, Edinburgh](https://www.openair.hosted.york.ac.uk/?page_id=525) | Andrew Chadwick and Simon Shelley (OpenAIR) | `b-format/middle_tunnel_1way_bformat.wav` |
| `mausoleum-hamilton` | [Hamilton Mausoleum, Hamilton, Scotland](https://www.openair.hosted.york.ac.uk/?page_id=502) | Audiolab, University of York: Damian T. Murphy (OpenAIR) | `stereo/hm2_000_ortf_48k.wav` |
| `spring-ak-springer` | [AKIR Springer, setting 10](https://www.adventurekid.se/akrt/free-reverb-impulse-responses/) | Adventure Kid | `AKIR_Springer.zip: AKIR_Springer_10.wav` |
| `spring-ak-smooth` | [AKIR Smooth Springer, setting 10](https://www.adventurekid.se/akrt/free-reverb-impulse-responses/) | Adventure Kid | `AKIR_SmSpringer.zip: AKIR_SmSpringer_10.wav` |
| `spring-ak-dual` | [AKIR Dual Springer, setting 8](https://www.adventurekid.se/akrt/free-reverb-impulse-responses/) | Adventure Kid | `AKIR_DualSpringer.zip: AKIR_DualSpringer_08.wav` |

Adventure Kid spring reverbs: the [free reverb impulse responses page](https://www.adventurekid.se/akrt/free-reverb-impulse-responses/)
marks each pack "licensed under a Creative Commons Attribution 4.0
International License" and asks: "If the IR's are redistributed in any way
such as a download or in a software, please give me a credit somewhere."
The page says the spring packs were made with the author's own plugin, so
they are not captures of another maker's hardware.

### What we left out, and why
- **Adventure Kid small rooms:** the page says they were made "from real
  rooms and reverb units" without saying which file is which. Some may be
  captures of another maker's reverb unit, so we skip the whole set.
- **Adventure Kid speakers and amps:** cabinet captures, not rooms. Left for
  the amp simulator work to decide.
- **Usina del Arte Symphony Hall (OpenAIR):** its page shows no licence.
- **Aachen Impulse Response database (MIT):** the download server refused
  connections from our build machine on the day. It can be added later; copy
  the copyright line from the LICENSE file in the download.
- **R1 Nuclear Reactor Hall** is credited to "OpenAIR, Audiolab, University
  of York" because its page lists no names.

## Consequences
- The library adds about 13 MB (15 rooms, 48 kHz 16-bit stereo, at most 10 s each) to the app.
- Adding a room means: check the licence on its source page, convert it with
  `convert_room`, add a `library.rs` entry, and add a row above.

## Sources
- OpenAIR about page on licences: https://www.openair.hosted.york.ac.uk/?page_id=2
- Each space's page, linked in the table.
- Adventure Kid: https://www.adventurekid.se/akrt/free-reverb-impulse-responses/
- Creative Commons Attribution 4.0: https://creativecommons.org/licenses/by/4.0/
