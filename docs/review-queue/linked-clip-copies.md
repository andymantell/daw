# Copied clips are linked instances

From the duplicate-clips discussion after song test 3. Copying a clip
(Duplicate, copy and paste, or an assistant's `duplicate_clip`) now
makes a linked copy: an instance of the same clip. Editing one changes
them all; Unlink breaks one off.

Screenshots (in the project's screenshots folder), from a song-3 style
arrangement built through the assistant interface:
`app-linked-before.png` (separate copies, as song 3 had them),
`app-linked-after.png` (the same arrangement as linked copies) and
`app-linked-closer.png` (closer in, with one Sub Bass instance picked:
its other instances are outlined in the group's colour).

- [ ] **Duplicate a note clip.** Right-click a note clip →
      **Duplicate (linked)**. Pass: both clips get a coloured band along
      the top, a link mark and "×2"; adding a note to either in the note
      editor shows up in both.
- [ ] **Its own place.** Move, lengthen, trim or mute one of them.
      Pass: the other stays where and as it was.
- [ ] **A separate copy.** Right-click → **Duplicate as a separate
      copy**. Pass: no band; editing it leaves the original alone.
- [ ] **Unlink.** Right-click a linked clip → **Unlink** (or Edit →
      Unlink clips with it selected). Pass: its band goes; editing it
      no longer changes the others. Unlinking one of a pair unlinks
      both.
- [ ] **Audio clips.** Copy an audio clip and paste it. Change the
      pasted one's gain or transpose. Pass: the original changes too;
      its fades and trim don't.
- [ ] **Undo.** Edit the notes of one linked copy, then Undo. Pass: one
      undo puts every copy back.
- [ ] **Older projects** open with every clip on its own, as before.

## Decisions to accept

- [x] **What linked copies share.** Note clips: notes, controller
      lanes, loop, key and name. Audio clips: the audio, name, gain,
      volume line, transpose and pitch edits. Each copy keeps its own
      place, length, trim, fades, stretch markers and mute, so a copy
      can be split, trimmed or faded without touching the others (a
      split would otherwise remove the fades of every copy).
- [x] **Copies are linked by default** everywhere a clip is copied
      (Duplicate, paste, `duplicate_clip`); `linked: false` and
      "Duplicate as a separate copy" make one of its own. Duplicating a
      section or a track version copies clips as they are (a linked
      clip stays in its group).
- [x] **Pattern clips** keep linking through their pattern, as before,
      and are drawn as a group the same way.
- [x] **How a group is drawn:** a colour per group, picked from it, as a
      band along the top of each instance, with a link mark and the
      number of instances where the clip is wide enough. The other
      instances of a picked clip are outlined in that colour.
