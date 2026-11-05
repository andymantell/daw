# UI polish, step five: the classic poly synths on the panel kit

Step five of the UI polish plan (`docs/plans/ui-polish.md`, section 5)
for the three classic poly synths: the June, the Oracle and the Eighty.
Each is now a three-unit plate in the rack, laid out on the panel kit in
the same grammar as the poly-synth (modules left to right in signal
order), with its own modules from its own architecture. The panels are
in `crates/app/src/poly_panels.rs`.

## The panels

- **June**: Oscillator (range, what moves the pulse width, width,
  movement, LFO to pitch, and the mixer: saw, pulse, sub, noise),
  Filter (the stepped high-pass as a switch, the response drawn live,
  the large cutoff, resonance, envelope, LFO, keys), Envelope (level
  follows the envelope or the gate, its shape drawn), LFO and chorus,
  Playing (poly / unison / mono, portamento, voices, detune, glide,
  wheel, bend, velocity), Output (level, drive, drift).
- **Oracle**: Osc A and Osc B (the waves as lit buttons, since the
  synth adds them together; sync, low frequency, keys), Mixer,
  Poly-mod (its two sources and three routes), Filter (board A or B,
  board B's output, the response drawn for the board and output
  chosen, the large cutoff, keyboard tracking), Envelopes (filter and
  level, each with its velocity knob), LFO (rate, amount, the shapes it
  sums, and its routes), Playing.
- **Eighty**: Channel 1 and Channel 2 side by side, each in three rows
  (oscillator; the high-pass, low-pass with its slope, and the filter
  envelope's levels; the filter and level envelopes), Mix and touch
  (velocity and aftertouch), Modulation (the sub-oscillator and the
  ring modulator), Playing.

## Small changes to the kit

- Knob readouts in octaves show one decimal place (`+0.8 oct`), so two
  side by side don't run together. The settings grids and the assistant
  interface still give two.
- The filter display can draw a notch, for the Oracle's board B.

## For the AI assistants

Every knob, switch and button is a setting of its synth, reached by
`set_parameter` as before (each marked `// ai: set_parameter`); the
header's sound buttons are `next-sound`, `previous-sound` and
`compare-sound`, and `get_instrument` gives the sound and the rack view.
End-to-end tests: `poly_panels::tests`, one per synth, click a control
and read the setting back through the assistant interface, set a setting
by tool, and step the sounds by click and by command.

## Sizes

At 1920 x 1200 the June fits the dock with the inspector open; the
Oracle and the Eighty scroll sideways a little with the inspector open
(about 1.7 thousand points of modules) and fit with it closed. At
1440 x 900 all three scroll sideways, as the poly-synth does.

## Shots

- `ui-polish-5-poly-synths/june-1920-valve-glow.png`,
  `oracle-1920-valve-glow.png`, `eighty-1920-valve-glow.png`
- `june-1920-vellum.png`, `oracle-1920-tidal.png`,
  `eighty-1920-workshop.png`: the other themes
- `oracle-1440-valve-glow.png`

## Sign-off items

None open.
