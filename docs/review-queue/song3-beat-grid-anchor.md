# Song test 3 wish list 1 and 9: beat grid of an a cappella, and lining a clip's point up with a bar

What was built: `analyse` with `beats: true` finds the tempos a voice's
note starts fit (with confidence, first beat and first downbeat), and
`edit_clip anchor_seconds` moves an audio clip so a chosen point of it
lands on a bar, a time, or the nearest beat or bar. On the timeline, the
clip menu and the Edit menu have "Line up playhead point with nearest
beat" and "... nearest bar".

On the real a cappella from song test 3 (Ten Letters, "Profit", CC0), the
beat grid gives 116.5 bpm with confidence 0.98, where `analyse` tempo
said nothing. The guess of which beat is the downbeat was unsure there
(0.11), as it says.

- [ ] Import an a cappella, select its clip, put the playhead on the start
  of a sung phrase, and choose "Line up playhead point with nearest bar"
  in the clip's right-click menu. Pass: the phrase now starts on the bar
  line, and one undo puts it back.
- [ ] Same with "nearest beat" on a note in the middle of a phrase.
  Pass: that note is on the beat line.
- [ ] Decide whether the beat grid should also be in the app itself (for
  example a "Guess from beats" button in the clip tempo window). For now
  it is an assistant tool only.
