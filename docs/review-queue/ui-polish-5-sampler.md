# UI polish, step five: the sampler's panel

Part of step five of the UI polish plan (`docs/plans/ui-polish.md`,
section 5). The sampler now sits in the rack as a three-unit panel on
the panel kit, in place of its settings grid. Everything it did before
still works.

## What changed

On its own sound files, the modules run left to right in signal order:

- **Key map**: every zone drawn as a box over the keys it plays (across)
  and how hard they are struck (up), with its root note marked and a
  keyboard along the bottom. Velocity layers stack; round-robin zones
  sit on top of each other. Click a zone to show it; hover for its keys
  and velocities. Under it: Add sound files, Spread, Normalise, and Load
  an SFZ library.
- **Zone**: the zone shown (previous, a list, next), its sound, Remove;
  then knobs for root, lowest and highest key, softest and hardest
  velocity, round-robin turn, tune, level, pan, the loop switch and the
  loop's start and end.
- **Filter**: low-, band- or high-pass as a switch, its response drawn
  live, the big cutoff knob and resonance.
- **Envelope**: its shape drawn, attack, decay, sustain, release.
- **Playing**: velocity, bend range, level.

With an SFZ library loaded, the panel shows what plays it instead:

- **Library**: its name in the badge lettering, its size, any problems
  and settings not played yet (hover), Load another, Back to zones.
- **Articulation**: the key switches as a list (only when it has any).
- **Controls**: a knob for each controller it names, such as mic levels.
- **Output**: level and normalise.

A sampler inside a layered instrument keeps the settings grid, as the
poly-synth does.

## For the AI assistants

- Every knob and switch is a setting (`set_parameter`), as before; the
  buttons are the same commands and `edit_instrument` / `set_sound`
  arguments as before.
- `get_instrument` now lists a sampler's `zones` when it plays its own
  sound files: each with its sound, name, root, lowest and highest key,
  softest and hardest velocity, loop and round-robin turn, and `shown`,
  the zone the panel shows. That is what the key map draws.
- End-to-end tests: `sampler_panel::panel_tests` drive the panel by click
  and by tool, on zones and on a loaded library.

## Sizes

The modules are about 1400 points wide on zones and less on a library,
so at 1440 × 900 on zones the plate scrolls sideways, as the
poly-synth's does. At 1920 × 1200 it is all in view.

## Renders

Here: `ui-polish-5-sampler/sampler-1920-valve-glow.png` (zones, whole
window) and `sampler-1440-sfz.png` (a library loaded). In the project
files, `ui-polish/sampler/`: `sampler-1920-*.png` and
`sampler-1440-*.png` in each theme and in light mode, and
`sampler-*-sfz.png` with a library loaded. Recipe:
`ui-polish/readme_shots_with_sampler.rs` (test `sampler_review`).

## Sign-off items

None open.
