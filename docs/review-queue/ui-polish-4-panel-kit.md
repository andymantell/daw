# UI polish, step four: the panel kit and the rack

Step four of the UI polish plan (`docs/plans/ui-polish.md`, section 5,
"Instruments and effects get front panels" and "A hint of a rack"). The
kit is built and the poly-synth is the first instrument on it; the other
instruments (step five) and the effects keep their settings grids inside
the rack until their own threads.

## What changed

**One panel kit** (`crates/app/src/panel_kit.rs`) that every built-in
instrument's panel is drawn from:

- **Knobs**: drag up or right, Shift for fine steps, the wheel and the
  arrow keys step them, a double-click puts one back to its default. Each
  has its label and its value in units under it (Hz, ms, dB, semitones,
  cents, octaves, percent). Three sizes: small for an envelope's four,
  normal, and large for the control that matters most.
- **Switches**: a row of segments for a small choice (a wave shape, a
  play mode, the number of unison voices), in place of a drop-down. A
  long list (the eight filter types) stays a drop-down.
- **Modules**: framed blocks with a heading, laid left to right in signal
  order.
- **Displays**: the filter's response and each envelope's shape, drawn
  from the settings as the knobs turn.
- **A header** on each unit: its name in the theme's heading face, a
  line on what it is, the sound with previous and next, Save (the track
  preset) and Compare, and on the right the "Notes in" buttons (note
  effects, note repeat, MIDI output), each opening its controls.

**The hint of a rack**: a rail down each side of the Instrument tab with
mounting holes; each unit a front plate a shade apart from the dock,
with a light along its top edge, a shade under its bottom edge, and
an ear with a screw at each corner. A unit on the kit has a fixed
height in rack units (the poly-synth is three); one still on its
settings grid fits its contents. The rail's menu, "Show in rack", lists
the track's units (the instrument and each effect) with a tick each and
"as a strip", and Show all. A collapsed unit is a strip with its name
plate, its sound and a level meter; a hidden one still plays. The
choice is kept per track in the project.

**Per theme**: the rail, plate, edge light, shade, screw and badge
colours are theme tokens, with a `[rack]` section for the corner
radius, line weight and the plate's surface drawn by code: brushed
hairlines in Valve Glow, plain in Workshop and Tidal, a ruled card in
Vellum. No image textures, no 3D caps.

**The poly-synth** got six sounds (Warm pad, Bright lead, Round bass,
Pluck, Strings, Hollow keys) so the header's sound buttons have
something to step through.

## For the AI assistants

- `set_parameter` moves every knob and switch, as before.
- `get_instrument` gives `sound` (the sound the settings are at, if one
  of its sounds) and `rack` (each unit with its view).
- New tool `set_rack` (track, unit: `instrument`, an effect's id or
  `all`; view: shown, collapsed or hidden).
- New commands `next-sound`, `previous-sound`, `compare-sound`.
- End-to-end test: `instrument::kit_tests` drives the panel by click
  and by tool and checks each sees the other.

## Sizes

The poly-synth's unit fits the dock at 1440 × 900 with both side panels
open; its modules are about 1150 points wide, so at that width the
plate scrolls sideways a little (collapse a side panel and it fits).
At 1920 × 1200 everything is in view.

## Before and after

- `ui-polish-4-panel-kit/before-1920-instrument.png`,
  `before-1440-instrument.png`: the settings grid (the step-three pane).
- `1920-valve-glow.png`, `1440-valve-glow.png`: the poly-synth on the
  kit, whole window.
- `1920-vellum-dock.png`, `1920-tidal-dock.png`,
  `1440-workshop-dock.png`: the dock in the other themes.
- `1920-collapsed-dock.png`: the unit collapsed to a strip.

The render recipe is `ui-polish/readme_shots_with_kit.rs` in the
project files (test `kit_review`).

## Sign-off items

None open. Questions were asked in the thread.
