# Research: a desk-style mixer with vertical faders

Checked 2026-11-01, for a feature request: the mixer should have vertical
faders and be reminiscent of a real mixing desk.

## Where we are

The mixer pane today (`crates/app/src/mixer.rs`) is a table: one row per
track, with a horizontal level meter, a horizontal volume slider, a pan
knob-style slider, Mute, Solo, FX, Out, Sends, Group and Take columns, a
master row and a control-group list underneath. It works, and every
control is already reachable by keyboard and screen reader, but it reads
as a settings grid, not a desk.

## What makes a mixer read as a desk

Taken from how hardware consoles are laid out; the same order appears on
most of them, which is why players find it familiar:

- **One narrow vertical strip per channel**, side by side, in the same
  order as the timeline. Reading top to bottom: input and routing, insert
  slots, sends, pan, then the fader with its meter beside it, then mute,
  solo and arm, then a name tag ("scribble strip") in the track colour at
  the bottom. Buses and folders sit to the right of the tracks that feed
  them, the master strip on the far right, control groups beside it.
- **A long-throw vertical fader** with a dB scale printed beside it and
  a marked, slightly sticky **0 dB (unity) position**. Travel is not
  linear in dB: most of the throw covers the range that matters (about
  -20 dB to +6 dB) and the bottom few millimetres cover -20 dB to
  silence. In software this is a curve from fader position to gain, with
  the top at +6 dB or +12 dB and unity at roughly two thirds to three
  quarters of the way up.
- **A level meter alongside the fader**, the same height as its travel,
  with peak hold, a clip light that stays until clicked, and the scale
  shared between fader and meter so the two can be read together.
- **A pan pot above the fader** with a centre detent.
- **Lit buttons** for mute, solo and arm, with the colours the rest of
  the app uses for those states.
- **Enough strips on screen** that a band mix fits: strips of about 70
  to 90 pixels wide, with a narrow mode (fader, meter, name only) for
  large projects, and horizontal scrolling with the master strip pinned.

## How it works as software

- **Mouse:** drag the fader cap; the pointer stays on the cap (no
  jumping to the click point), a modifier gives fine control, scroll
  wheel nudges by small dB steps, double-click returns to unity, and
  Alt-drag or similar moves a selection of faders together, keeping
  their relative levels. The same for pan.
- **Keyboard and screen reader:** the strip is a column of the same
  commands as now (volume in dB, pan, mute, solo, arm, out, sends), so the
  accessible names and the spoken level and pan stay as they are. The
  fader is a slider with a value in dB; typing a number into it still
  works.
- **Theme:** the fader cap, the track, the scale, meter segments and the
  scribble strip are drawn from theme tokens, so each of the four themes
  gets its own desk look (brushed metal and amber in Valve Glow, paper and
  ink in Vellum, and so on). The theme research page lists the mixer as a
  view still to be designed per theme; this is that design.
- **The toolkit** (egui 0.37) has a vertical slider (`Slider::vertical`),
  custom value formatting, a logarithmic mode and a trailing fill, so a
  plain vertical fader costs little. The desk look (cap, scale, detent,
  meter beside it, fader law) is our own widget drawn with the painter,
  which the meters already are. *(Source: the egui 0.37 Slider
  documentation, read 2026-11-01.)*
- **Agents:** the strip changes nothing in the command layer (2.11);
  volume, pan, mute, solo and routing remain the same commands.

## Not researched yet

- Whether to keep the current table as an alternative "list" view for
  routing-heavy work, or fold routing into the strip's top section only.
- Touch: fader throw on a touch screen or the phone remote (the remote
  already plans meters; its faders can share the same law).
