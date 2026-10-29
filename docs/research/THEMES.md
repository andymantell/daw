# Research: themes and look and feel

Started 2026-10-29. Mock-ups were made as browser pages showing the same
lesson project in each look, so they could be compared side by side.

## Round 1: four directions

| Direction | Feel | Verdict |
|---|---|---|
| Valve Glow | Warm dark browns, amber light, cream text; the playhead glows | **Chosen** as the base; variations explored in round 2 |
| Practice Room | Light, friendly, rounded, a typeface built for legibility | Not pursued |
| Stage Light | Near-black, solid colour clips, large readouts for distance | Not pursued |
| Drafting Table | Cool blue-greys, hairlines, outlined clips, fine beat grid | Variations explored in round 3 |

## Round 2: variations on Valve Glow

| Variation | Pushes | Verdict |
|---|---|---|
| Brushed Chassis | Depth: bevels, recessed timeline, inset readout | Not chosen |
| Tolex and Brass | Character: oxblood texture, brass, cream pill controls | Not chosen |
| Tube Trace | Glow: near-black, waveforms as glowing traces, needle meters | Not chosen |
| Workshop | Calm: warm matte greys, copper, layered panels with shadows | **Chosen** |

Mock-ups: a page of the four directions, one of Valve Glow variations, one
of Drafting Table variations and one of six new themes, kept outside the
repository.

## Round 3: variations on Drafting Table

| Variation | Pushes | Verdict |
|---|---|---|
| Blueprint | White lines on cyanotype blue, no track colours | Not chosen |
| Vellum | Warm tracing paper, ink-line waveforms, red pencil for live state | **Chosen** |
| Instrument Panel | Same drawing on dark graphite, colour only as data | Not chosen |
| Ledger | Black ink on white, print-like, no track colour | Not chosen |

## Round 4: six new themes

| Theme | Feel | Verdict |
|---|---|---|
| Observatory | Deep night blue, faint stars, pale cyan accent | Not chosen |
| Foundry | Charcoal, square corners, uppercase type, molten orange | Not chosen |
| Tidal | Pale sea-glass, very rounded, friendly typeface, single-line waveforms | **Chosen** |
| Letterpress | Cream card, deep ink, oxblood, halftone-dot waveforms | Not chosen |
| Phosphor | Green phosphor terminal, one green, scanlines | Not chosen |
| Chalkboard | Slate green, handwritten chalk text, dashed clip outlines | Not chosen (a possible lesson-mode look later) |

## Decision so far (2026-10-29)

Ship **Valve Glow**, **Workshop**, **Vellum** and **Tidal** first, each
with a light and a dark variant, switchable at runtime.

## Built so far (2026-10-29)

The foundation: themes are files (`crates/app/themes/`, format in the
README there), egui's style is made from the active one, and the
timeline, waveforms, meters, mixer strips, tuner and status messages read
only theme colours. All four themes ship with both variants; the light
Valve Glow and Workshop and the dark Vellum and Tidal were derived, not
mocked up. A contrast test runs in CI. Bundled typefaces, time-of-day
switching and per-theme designs for dense views are still to come.

## Still to design

The mock-ups cover the timeline, transport bar and a row of mixer strips
only. Before the themes are built, each one needs the rest of the app
designed in it: the full mixer window, plugin and effect panels, the note
and notation editors, the browser, settings, dialogs and the phone remote.
Dense views are where a theme's spacing, line weights and contrast are
really tested.

## What a theme must define

Colour tokens (surfaces, text, lines, accent, record, playhead, loop,
comment pin, meter gradient, track palette), type (faces and sizes for
labels, numerals and headings), shape (corner radii, line weights,
spacing), waveform style (filled, outlined, single line, glowing trace),
textures (drawn by code, no image assets) and glow (on or off, colour,
radius). Light and dark variants share everything but the colour tokens.

## Rules

- Original designs only; no reproduction of any product's look (CLEAN_ROOM).
- Fonts are bundled only under licences that allow it (SIL OFL or similar),
  with the licence text shipped alongside.
- Every theme passes a WCAG AA contrast check in CI, and live state must be
  readable without colour (shape or motion as well).
- Themes and window layouts are per computer, never stored in the project.
