# UI polish, step five: the drum machine on the panel kit

Part of step five of the UI polish plan (`docs/plans/ui-polish.md`,
section 5). The drum machine's settings grid is now a three-unit front
panel in the rack, drawn from the step-four kit.

## What changed

Modules left to right, as the sound goes:

- **Pads**: the sixteen pads, four by four with the lowest at the bottom
  left, as on hardware. Each shows its name and key, dimmed when it has
  no sound, with small marks for a pad with effects (`fx`) or an output
  of its own (`›3`). Click one to show it.
- **Kit**: the whole kit's level and how much velocity matters.
- **Pad**: the chosen pad's level, pan and tune knobs, its choke group as
  a row of segments (Off, 1 to 8), how it plays its notes, and a switch
  for what shows beside it: **Layers**, **Effects** or **Outputs**.
- **Layers**: a module per layer: what it plays (off, a sample, or a drum
  made here), the sound file for a sample, then level, tune, pan, decay,
  tone (or start, for a sample), and the softest and hardest notes it
  plays for. An off layer is a narrow module.
- **Effects**: "Add an effect", then a module per effect on the pad in
  order, each with an on/off switch, Remove, and its settings as knobs
  (switches for small choices).
- **Outputs**: the pad's output (the track, or 1 to 16) and its two
  sends, and where each output in use goes, with "Put each pad on a
  bus".

Every feature of the old grid is still there. The pad and part shown now
survive adding or removing a pad effect (the panel used to jump back to
pad 1).

## For the AI assistants

- Every knob and switch is a setting, so `set_parameter` moves it, as
  before; `set_sound`, `edit_pad_effect` and `set_instrument_output` as
  before.
- `get_instrument` gives `pads` for a drum machine: each pad's key,
  name, layers playing, output and number of effects, and `shown` (the
  pad and part the panel shows).
- New tool `show_pad` (track, pad 0 to 15, part: layers, effects or
  outputs): the view only.
- End-to-end test: `drum_panel::tests` drives the panel by click and by
  tool and checks each sees the other.

## Sizes

At 1920 × 1200 the whole panel is in view (layers, effects or outputs).
At 1440 × 900 with both side panels open, the layers scroll sideways:
the first two layers show, layers 3 and 4 are a scroll away. Pad names
longer than a pad (Closed hi-hat, in the wider-lettered themes) are cut
with an ellipsis; hovering gives the full name.

## Before and after

In `ui-polish-5-drum-machine/`:

- `before-1920-dock.png`: the settings grid it replaces.
- `1920-valve-glow-dock.png`: pad 3 (the snare, layered with a clap).
- `1920-effects-dock.png`: an EQ and a compressor on the snare.
- `1920-outputs-dock.png`: after "Put each pad on a bus".
- `1920-vellum-dock.png`, `1920-tidal-dock.png`, `1440-workshop-dock.png`:
  the other themes.
- `1440-valve-glow.png`: the whole window at 1440 × 900.
- `1920-collapsed-dock.png`: the unit as a strip.

Every theme at both sizes, whole window and dock, is in the project
files, `ui-polish/drum-machine/`. The render recipe is
`ui-polish/readme_shots_with_drums.rs` there (test `drum_review`).

## Sign-off items

None open.
