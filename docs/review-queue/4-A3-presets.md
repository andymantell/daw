# Phase 4 A3 Track and effect chain presets

Screenshots (in the project's screenshots folder): `app-presets.png`, `app-save-preset.png` and
`app-effects-presets.png`. (In the cloud screenshots some symbols show as
boxes, as the renderer there lacks those glyphs.)

- [ ] **Save a track as a preset.** Set up a guitar track with the amp,
      a cabinet (try one of your own impulse responses, or an amp
      capture), a delay and a send to a reverb bus, and turn it down a
      little. Right-click the track on the timeline → Save track as
      preset… (or File → Save track as preset…). Give it a name and the
      tags "guitar, crunch", Save. Pass: it is in
      `C:\Users\<you>\DAW\Presets` as `<name>.dawpreset`, and File →
      Presets… lists it under Yours, with its tags.
- [ ] **Add it to another project.** Start a new project with a bus
      called the same as the reverb bus. File → Presets…, type "crunch" in
      Find (or pick the tag), pick it, Add as a new track. Pass: a track
      with the same amp, cabinet (your own file plays, though it was never
      copied by hand), delay, volume and pan, sending to the bus of that
      name; no clips. In a project without that bus, the message says the
      send was left out. Edit → Undo takes the track away in one step.
      Save the project, then check the cabinet's file is in its Audio
      folder.
- [ ] **Put a chain on a track.** On a track with a recorded take, open
      its effects (FX), press Presets…, pick an effect chain (say Clean
      guitar), Put on. Pass: the track's effects are replaced, its take
      stays. Save as preset… in the effects window saves just the chain.
      A plugin's settings come back as you saved them.
- [ ] **Send a tone to someone.** In Presets…, pick your preset, Export…,
      and send the file to the guitar teacher (or copy it to another
      computer). There, drop the file on the app (or File → Import
      preset…). Pass: it is listed under Yours, and added to a project it
      sounds the same, files and all.

## Decisions to accept

- [x] **Which tones ship as presets.** Built as placeholders, all from
      the built-in effects: *Clean guitar*, *Crunch guitar*, *High gain
      rhythm* (effect chains on the amp), *Bass DI*, *Vocal*, and a
      *Reverb bus* track preset. Say which to keep, drop or add, and how
      each should be set; your own once you have dialled them in can be
      exported and put in the app as they are.
- [x] **What a track preset keeps.** Its instrument (or layers), effects,
      amp, sends, output bus, instrument outputs, volume, pan and
      notation settings; not its clips, notes, takes, comps, automation,
      frozen audio, mute or solo. Sends, the output and sidechains are
      kept by the other track's name, and found by name where it is
      added (left out, with a message, where there is none). Say if mute
      and solo, or the automation, should be kept too.
- [x] **Putting a track preset on an existing track.** It replaces the
      instrument, effects, sends and settings and keeps the track's name,
      clips and notes. A bus preset only goes on a bus (or as a new
      track). The alternative is for presets only ever to add new tracks.
- [x] **How presets travel.** A preset file is a bundle, like a project
      bundle: a plain tar holding the one-track document, a short text
      with its kind, tags and description, and the sound files its
      effects and instruments use. So "presets travel in bundles" here
      means a preset is its own small bundle. A project bundle doesn't
      carry the presets in your library (the tracks in it already have
      their settings). Say if it should.
- [x] **Importing a preset with a name you already have** keeps both,
      the new one as "Name 2", rather than replacing yours.
