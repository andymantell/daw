# Built-in drum kit profiles

Each `*.kit.txt` file here is a drum kit profile built into the app
(roadmap 3.4). Profiles are learned with **Settings → Drum kits → Learn a
kit**, then exported and added here, so that kit needs no learning on any
other computer.

To add one, copy the exported file into this folder and list it in
`BUILT_IN` in `src/kit.rs` with `include_str!`. Keep the `port` lines: the
app offers the profile when a MIDI input with that name is connected.
