# Phase 4 (plan A5) Parts in from another project, and parts out

Screenshots: `/mnt/project-files/screenshots/app-import-from-project.png`,
`app-imported-tracks.png`, `app-export-parts-project.png` and
`app-export-stem-package.png`.

Built and tested in the cloud: tracks come in with new ids (so the same
part can come in twice), their audio is copied into this project's
folder (or played from the other project's until an unsaved one is
saved), sends, sidechains and buses left behind are let go of, frozen
audio still fits, a project at another sample rate keeps its timing, and
one Undo takes the whole import back. Assistants can do the same through
the `import_from_project`, `export_as_project` and `export_stem_package`
tools.

- [ ] **A lesson part into a student's project.** Make "Lesson 1" with a
      backing track (with an effect or two), sections and a tempo. In a
      new project, File → Import from project…, choose Lesson 1, tick the
      backing track, Sections and Tempo, and press Bring in. Pass: the
      track plays as it did in Lesson 1, with its effects; the sections
      and tempo are there; Edit → Undo takes it all away at once.
- [ ] **A plugin's settings come along.** Import a track with a plugin
      effect set away from its defaults. Pass: the plugin opens with the
      same settings.
- [ ] **Export tracks as a project.** File → Export tracks as a
      project…, tick one track, choose bars 5 to 8, and export. Open the
      new project. Pass: it starts with what was in bar 5, at the tempo
      bar 5 had, with that track only, and its sections moved to match.
- [ ] **Stem package into another program.** File → Export stem
      package…, tick an audio track and an instrument track, the whole
      song. Drop the WAV files and the MIDI file into another DAW at bar
      1 with the tempo from "About these stems.txt". Pass: everything
      lines up. Try "Packed into one file (.tar)" too: the archive opens
      in Windows' own archive support (or 7-Zip).
- [ ] **Decide:** the MIDI files keep only the tempo at the start, as
      the page with the stems says. Is that enough, or should they carry
      the song's tempo changes as well?
- [ ] **Decide:** a cut tempo starts at the tempo in force at the first
      bar; a tempo that glides across that bar starts at the glide's
      starting tempo. Fine as it is?
