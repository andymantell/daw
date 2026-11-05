# UI polish, step five: front panels for the semi-modular, wavetable and FM synths

Part of step five of the UI polish plan (`docs/plans/ui-polish.md`,
section 5). Patch Twenty, Blue Wave and Lucky Seven now sit in the rack
on the panel kit, three units high like the poly-synth, in place of
their settings grids. Each keeps the kit's grammar (modules left to
right in signal order, knobs with values in units, switches for small
choices, live displays) and shows what its own architecture is about,
as the plan asks: the semi-modular its patch points, the wavetable synth
its table, the FM synth its operator routing.

## What changed

**Patch Twenty** (`crates/app/src/patch_twenty_panel.rs`): Oscillators
(waves and ranges as switches, levels, width, pitch modulation, glide);
Mod gen with its shape drawn (falling saw to triangle to rising saw);
Filters, the high-pass into the low-pass with their joint response drawn
and the drive in front; Envelopes (delay-attack-release and
hold-ADSR, both drawn); Amplifier and input (what the amplifier follows,
drone, level, the external input and its trigger); **Patch points**, a
small diagram of the jacks the four cords come from and go to, with a
line for each cord in use (brighter the more it carries, dashed when it
turns things down), and each cord's from, to and amount under it.
Drag from a jack on top to one below to patch a cord (the first free
one of the four, at half amount); drag a cord's lower end to another
jack to move it, or off the jacks to unplug it. Then Playing.

**Blue Wave** (`crates/app/src/blue_wave_panel.rs`): **Wavetable**, the
table and character, with the waves oscillators A and B read drawn from
the table itself (smooth or eight-bit as chosen) and a strip of the 64
places with where each reads; Oscillator B; Wave envelope; Filter (24 dB
response drawn, large cutoff); Envelopes (filter and level); LFO;
Playing.

**Lucky Seven** (`crates/app/src/lucky_seven_panel.rs`): **Algorithm**,
drawn as boxes for the six operators, the heard ones (carriers) filled,
lines from each modulator to what it modulates, the feedback loop, and
the operator being edited ringed; **Operators**, all six side by side
with each one's envelope shape, level knob and frequency (as a ratio or
in Hz); **Operator N**, the one picked with "Edit 1 to 6" in full: its
frequency, and either its envelope's four rates and levels (drawn) or
its keyboard scaling and touch; LFO and pitch envelope; Playing, with
"Load a bank…" and the bank's sounds once one is loaded.

**Kit additions** (`panel_kit.rs`): `param_switch_as` (a switch with
shorter segment labels than the setting's own, so "Tri Saw Pulse Noise"
fits), `param_combo` (a narrow drop-down for a long list), and the
screen and trace helpers are shared so a panel can draw its own
display. The header's line on what a unit is now gives way to the
buttons, cut short with the whole line on hover, so long descriptions
no longer run under Save and Compare.

## Fit

At 1920 × 1200 with the instrument pane full width all three fit with
room to spare. In the dock with the inspector open, Lucky Seven's last
module scrolls sideways a little at 1920, and all three scroll sideways
at 1440 × 900 (the poly-synth does too, with both side panels open).

## For the AI assistants

Every knob and switch is a setting of the synth, reached with
`set_parameter` as before; `list_parameters` and `get_instrument` read
them. Which Lucky Seven operator is being edited, and its page, are only
the view (marked `// ai: view`). No new tools or commands. End-to-end
tests drive each panel by click and by assistant:
`patch_twenty_panel::tests`, `blue_wave_panel::tests`,
`lucky_seven_panel::e2e` (and a test that every one of the 32
algorithms lays its operators out apart).

## For the owner to look at

Screenshots in `ui-polish-5-digital-synths/`: each panel at 1920 (cropped
to the rack), each in the dock at 1440 × 900, and one per theme
(Vellum, Tidal, Workshop). The full set is in the project files under
`ui-polish/digital-synths/`.

- [ ] The three panels read as three instruments, in the kit's grammar.
- [x] Patch points as a jack diagram, patched by click and drag
      (owner, 2026-11-05: "Click and drag").
- [x] Lucky Seven: six operators side by side plus one in full, picked
      with "Edit" (owner chose keep it as built, 2026-11-05).
- [ ] Sideways scrolling at 1440 × 900 in the dock is acceptable.
