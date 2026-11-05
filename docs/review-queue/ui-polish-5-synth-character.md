# UI polish, step five: each synth as its own piece of hardware

Step five of the UI polish plan (`docs/plans/ui-polish.md`, section 5,
"the nine read as nine instruments"). Each classic synth now has its own
visual character on the shared panel kit, so it reads as its own unit in
the one rack: the owner asked for more than a tint of the theme, so the
unit is the synth's own colours and the rack around it stays the theme's.
The characters are in `crates/app/src/character.rs`.

## What a character is

- **A palette**: the front plate, lettering and dim lettering, an
  accent, a screen's glass and ink, knob caps and their pointers, and
  wooden side cheeks where its kind had them. A test checks every
  pairing reads (lettering on plate, accent on plate, ink on glass,
  pointer on cap, lettering on buttons).
- **A knob style**: the kit's cap, skirted with tick marks, a wedge
  pointer, a dot on a flat cap, or a flat ring.
- **A switch style**: the kit's segments, lit buttons with a lamp, or a
  slide switch.
- **A module style**: the kit's framed block, bare lines on the plate,
  a filled heading band, or a thin boxed frame with the heading set
  into it.
- **A screen style**: a scope, glowing lamps, or a liquid-crystal panel
  with a dot grid.
- **The name plate's lettering** (capitals, wide-spaced, lower case,
  the readout face, or a badge), a **band** across the top (a stripe,
  section colours, a pinstripe, or patch points) and its own **corner
  radius** (sharp on the digital units, rounder on the older ones).

The choices evoke each synth's era and type; none copies a real
product's livery, badge or lettering. The house poly-synth keeps the
kit's plain look.

## How it is applied

The character in force is kept in frame data while a unit draws, and
`theme::look` returns the theme's colours with the character's swapped
in, so the kit's widgets, egui's own widgets inside the unit and the
panels' own drawings all take it without naming it. On a colour-coded
panel the module headings take the section colours in turn.

## For the AI assistants

`get_instrument` reports `instrument.character` (knob, switch, module,
screen, lettering, band, plate, accent, cheeks). End-to-end test:
`instrument::kit_tests::a_synths_character_draws_on_its_unit_and_an_assistant_reads_it`.
Screen readers still hear the name plate as the unit's name in capitals.

## Shots

- `ui-polish-5-synth-character/six-units-valve-glow.png`: Model Mono,
  Acid Three, Oh-One, June, Eighty, Oracle.
- `four-units-valve-glow.png`: Patch Twenty, Blue Wave, Lucky Seven,
  and the plain poly-synth.
- `all-units-vellum.png`: all ten in the paper theme, where the units
  keep their own colours and only the rack changes.

## Sign-off items

Left for the owner's call after the review of the renders: the wooden
cheeks are only as wide as the mounting ears; the header row (Sound,
Save, Compare, Notes in) is the kit's same row on every unit; and all
readouts share one numerals face, where the digital units could show
theirs in small liquid-crystal boxes.
