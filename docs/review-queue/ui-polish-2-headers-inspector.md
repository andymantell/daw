# UI polish, step two: track headers and the inspector

Step two of the UI polish plan (`docs/plans/ui-polish.md`, section 1:
"Timeline with real track headers" and "Inspector on the right"). Nothing
was removed: the mixer keeps its own mute, solo and arm, and every
window opens as before.

## What changed

**Track headers** are wider (176 points, from 110) and show, left to
right: the track's colour, an icon for its kind (audio, instrument, bus,
folder), its name, lit **M**ute, **S**olo and a**R**m buttons, and a
small level meter at the edge. Buses and folders have no arm; nor does
an audio track with no input. A tall track puts the buttons on a second
row under the name; a short one keeps them beside it. The edit group's
name and the track version playing show after the buttons when there is
room. Clicking a header shows that track in the inspector and outlines
the header.

**The inspector** is a new pane on the right of the default layout, the
height of the window. It shows one track:

- Track: name and kind; Mute, Solo, Arm; Volume and Pan; Output and
  Sends.
- Instrument (instrument tracks): its name and Open; the sound, with a
  list and previous and next buttons. It says "Your own" once the
  settings no longer match a built-in sound.
- Effects: each effect on the track with its On switch, and Open for the
  effects window (where effects are added and reordered).
- Clip, when one on that track is selected: an audio clip's name, start,
  length, gain, fades and transpose, with editable gain, fades and
  transpose; a note clip's name, start, length and notes, with Edit
  notes (or Edit pattern). Linked copies say how many there are.

It follows what you pick last: a header, a clip, a note clip or a mixer
strip. With nothing picked it shows the first track. Close it from its
pane header or with View → Inspector (Ctrl+I); it can go full size or
into its own window like any pane. A layout saved before this change
gets the inspector added once; closing it after that keeps it closed.

**The browser** still opens on the left of the timeline (View → Content
browser, Ctrl+B), so the window reads browser, timeline, inspector.

**Simpler mode**: the headers keep M, S and R; the inspector shows only
volume, pan and the sound.

## For the AI assistants

- The `inspector` command shows or hides the pane.
- New tool `select_track` shows a track in the inspector.
- `get_selection` has an `inspector` field: whether it is open, the
  track it shows and the clip.
- Header and inspector buttons carry `// ai:` marks (`set_parameter` for
  mute, solo, volume and pan; `set_track_input` for arm; `edit_track`,
  `set_send`, `set_instrument`, `edit_effect`, `edit_clip` for the rest),
  with spoken names of their own ("Mute the Vocal track", "Track
  volume") so they don't clash with the mixer's.
- End-to-end test: `inspector::tests::headers_and_the_inspector_work_with_an_assistant`.

## Pictures

In `ui-polish-2-headers-inspector/`, at 1920 × 1200 and 1440 × 900
(the song-3 test project, the Valve Glow theme):

- `*-inspector-clip.png`: the vocal track (armed) with its clip
  selected; the bass soloed.
- `*-inspector-synth.png`: the bass track, a synth, in the inspector.
- `*-browser-left.png`: the browser open on the left.
- `*-simple.png`: the same in simpler mode.

The renders are made with `readme_shots_with_headers.rs` in the project
files (`ui-polish/`), test `headers_review`, on top of the readme-shots
patch.

## For Andy to decide

1. **Inspector width.** It takes about a sixth of the window (320 points
   at 1920, 240 at 1440). Wider would fit the sends and sound list more
   easily; narrower leaves more timeline. Drag the divider to try it.
2. **The browser is beside the timeline only**, above the mixer, as it
   was. Should it run the full height on the left, like the inspector on
   the right? (Step three, the dock, makes these fixed regions anyway.)
3. **Previous and next sound** use up and down arrows. Fine, or would
   left and right arrows read better?

## Known limits

- The headers' meters move only while playing or with a track armed,
  as the mixer's do.
- The inspector's sound list is for the built-in synths with sounds;
  the sampler, drum machine and plugins show their name and Open.
- At 1440 × 900 with the note editor open below the timeline and the
  mixer below that, the note editor's toolbar wraps and its grid gets
  cramped. Step three (the dock) moves the editors into dock tabs.
