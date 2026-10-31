# Interchange fixtures

Project files laid out the way other DAWs write the open exchange format,
written by hand for the tests from the format's published schema and
reference. The tests package each one into a `.dawproject` file with a
short generated WAV file and, where needed, a generated plugin preset.

- `nested-clips.xml`: an audio clip inside a clip, cut by the clip around
  it, with a fade in beats.
- `looped-notes.xml`: a note clip whose loop plays two and a half times.
- `elsewhere.xml`: a mixer with a return channel, the generic compressor
  and EQ, plugins (one missing here, one in a format we don't host), a
  device of another DAW's own, warped audio, held automation, notes
  straight on a track, a marker between bars, tempo and time signature
  changes, and a scene.

Files exported from other DAWs join these only once their licence or
their contributor's permission allows them in the repository, with the
source and terms written beside them.
