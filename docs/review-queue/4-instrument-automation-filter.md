# Automating instrument settings, and a filter effect

Same `daw.exe`. Two things the assistant's third song test (deep house)
missed for filter sweeps. Built in the cloud; nothing here has been
listened to yet.

## Instrument settings on automation lanes

A built-in instrument's settings (the synth's cutoff, resonance, levels,
envelope, LFO and the rest, the sampler's, drum sampler's, drum
machine's and slicer's, and the simple synth's) can now have automation
lanes, as volume, pan, sends and effect settings already could. They are
in the automation pane's list of settings, named after the instrument
("Synth: Cutoff"), and assistants name them `instrument.<key>`, as
`list_parameters` and `set_parameter` already do. The engine sets the
instrument from the lane before each block it plays, so playback,
export, analyse and render in place follow them.
Writing works too: in Touch, Latch or Write, moving a knob in the
instrument's window while playing writes the lane. (A drum machine has
hundreds of settings, so only those that already have a lane are
watched for writing there.)

Not covered: plugin instruments (their parameters stay set from their
own window), and layered instruments (lanes are on the track's one
instrument).

- [ ] **Synth filter sweep.** On a pad playing chords, add a lane on
      "Synth: Cutoff" from about 300 Hz up to 10 kHz over four bars, and
      play it. Pass: a smooth sweep with no zipper noise or clicks, the
      same in an exported WAV.
- [ ] **Write a sweep.** Set the track's automation to Touch, play, and
      turn the synth's Cutoff knob. Pass: the lane gets the move, and
      after letting go the knob follows the lane again.
- [ ] **Lane removed.** Remove the lane while playing. Pass: the synth
      goes back to its knob's setting at once.

## Filter effect

A new built-in effect, **Filter**, under "Built in" in **Add effect…**:
low-pass, high-pass, band-pass or notch, cutoff from 20 Hz to 20 kHz,
resonance, 12 or 24 dB an octave, and a dry/wet mix. It is a
state-variable filter that stays stable while the cutoff moves, so it
suits lanes. At 24 dB the resonance is on the first of its two stages.
Most resonance is a strong peak (a Q of about 12), loud enough to need
the track's level brought down.

- [ ] **Low-pass sweep.** On a drum bus or a full mix, low-pass, 24 dB,
      Resonance 30%: sweep the cutoff from 200 Hz up to 20 kHz with a
      lane. Pass: from muffled to fully open with no clicks; wide open it
      sounds like the effect is off.
- [ ] **Resonance.** Turn Resonance up to 100% on a low-pass at 1 kHz.
      Pass: a clear, whistling peak, not harsh distortion or a runaway
      squeal. Say whether the top of the range should be lower or
      higher.
- [ ] **Other types.** High-pass at 300 Hz takes the low end out;
      band-pass leaves a telephone-like band; notch takes a narrow slice
      out. Pass: each sounds as named.
