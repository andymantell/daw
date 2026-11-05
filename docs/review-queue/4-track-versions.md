# 4 Track versions (Phase 4 plan, A11)

Same `daw.exe`. Screenshot from the cloud:
`/mnt/project-files/screenshots/app-track-versions.png` (the Track
versions window, and "Faster (2/3)" under the Lead track's name for the
version playing). Built without trying it on the PC.

A version holds what the track plays and how it sounds: its clips, note
clips, takes and comps, effects, instrument, automation, volume and pan
(and frozen audio). The track's name, mute and solo, routing, sends and
notation lane are shared by all its versions. A track has no versions
until one is made; the one it had becomes "Version 1".

- [ ] **Duplicate and compare.** Right-click an empty part of a guitar
      track, Versions → Duplicate this version. Pass: the name column
      shows "Version 2 (2/2)". Move a clip, change an effect setting and
      the fader, then pick Version 1 from the same menu. Pass: the clip,
      setting and fader are back as they were, with no gap or click
      longer than a moment, and Version 2 comes back as you left it.
- [ ] **New empty version.** Versions → New empty version. Pass: no
      clips, but the same effects, instrument and fader; record a new
      idea on it.
- [ ] **The window.** Edit → Track versions… Pass: each version is a
      choice to play, with Rename and Delete…; Duplicate and New empty at
      the bottom; the Track list picks another track.
- [ ] **Keys and command search.** Ctrl+K and type "version". Pass: Track
      versions…, Duplicate, New empty, Next, Previous and Delete track
      version are all there, and can be given keys in Settings → Key
      commands (none have keys out of the box).
- [ ] **Undo, save and reopen.** Switch, then Ctrl+Z. Pass: the version
      that played before plays again. Save, close and reopen. Pass: every
      version comes back, and the one that was playing plays.
- [ ] **Plugins keep their settings.** On a track with a plugin effect or
      instrument, duplicate the version, change a setting in the plugin's
      own window, switch back and forth. Pass: each version keeps its own
      plugin settings. (Plugins were not tried in the cloud.)
- [ ] **Sharing.** With a second copy of the project (bundle or live
      session), switch versions on one while moving a clip on the other.
      Pass: after merging, the moved clip is in the version it was moved
      in, and nothing is lost.

## Decisions to accept

- [x] **What a version holds.** Volume, pan, effects and instrument are
      in each version, as well as clips; name, mute, solo, routing, sends
      and the notation lane are shared. Say if the fader or effects should
      be shared instead.
- [x] **Commands act on one track.** Next, Previous, Duplicate, New empty
      and Delete act on the Track versions window's track while it is
      open, else the track of the selected clip or note clip, else the
      instrument window's track. There is no "switch every track to
      version 2" yet. Say if a song-wide switch is wanted.
- [x] **Deleting down to one version leaves none.** The track then plays
      the one left, as a track that never had versions.
- [x] **Not built yet:** showing two versions at once to copy clips
      between them, and versions on buses and folders (they have no clips,
      so the menu leaves them out).
