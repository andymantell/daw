# Automating plugin instruments and layers

Same `daw.exe`. The last open item from the third song test's wish
list: automation lanes reached a track's built-in instrument, but not a
plugin instrument or a layered instrument. Built in the cloud and tested
with the repo's test plugins; nothing here has been listened to yet.

## What changed

- **Plugin instruments.** A plugin instrument's parameters can have
  lanes, as a plugin effect's already could. They are in the automation
  pane's list of settings, named after the plugin ("Synth name: Cutoff"),
  and assistants name them `instrument.<parameter id>`, 0 to 1, as
  `list_parameters` lists them. The engine hands the plugin the value
  before each block, so export, analyse and render in place follow the
  lanes; the plugin's own window shows the value as it plays. Writing
  works as it does for plugin effects: in Touch, Latch or Write, moving a
  parameter that already has a lane in the plugin's window writes it.
- **Layers.** Each layer of a layered instrument has lanes for its
  level, its pan and its instrument's settings (a built-in instrument's
  settings or a plugin's parameters), named "Layer 1: Level",
  "Layer 1: Pan" and "Layer 1 Synth: Cutoff". Assistants name them
  `layer:<layer id>:gain`, `layer:<layer id>:pan` and
  `layer:<layer id>:instrument.<key>`. A lane leaves the layer's own
  slider where it is; the lane plays over it.
- When a track's instrument becomes the first layer (Add an instrument
  layer), its lanes move onto that layer and go on playing; Back to one
  instrument moves them back.

- [ ] **Plugin sweep.** On a plugin synth playing chords, add a lane on
      its filter cutoff over four bars, and play it. Pass: the sweep is
      heard, the plugin's window follows it, and an exported WAV has it
      too.
- [ ] **Write a plugin move.** With a lane on that cutoff, set the track
      to Touch, play, and turn the knob in the plugin's window. Pass: the
      lane gets the move, and a moment after letting go it follows the
      lane again.
- [ ] **Layer level ride.** On a track with two layers, add a lane on
      layer 2's level that brings it in over two bars. Pass: layer 2
      fades in smoothly, with no clicks, while layer 1 stays put.
- [ ] **Layer synth sweep.** Add a lane on one layer's synth cutoff.
      Pass: only that layer is swept.
