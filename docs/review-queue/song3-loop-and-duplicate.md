# Loop note clips and copy clips to many bars

From song test 3's wish list (items 11 and 14). A note clip can loop
its notes: lengthen it and the notes play again and again, and editing
them changes every time round. Copying a clip can put one copy on each
of a list of bars, or one every few bars.

- [ ] **Loop a note clip.** Right-click a note clip on the timeline and
      tick **Loop notes**, then drag its end out a few bars. Pass: the
      notes repeat in the clip (a small notch marks where each time
      round begins, and its name says "(loops)"), they play that way,
      and an edit in the note editor shows up in every repeat. The note
      editor draws a **Loop end** line; notes after it don't play.
- [ ] **Loop off.** Untick **Loop notes**. Pass: the notes play once
      and the rest of the clip is empty, as before.
- [ ] **A pattern clip.** Right-click a pattern clip. Pass: there is no
      Loop notes box (a pattern clip plays its pattern over and over
      already).

## Decisions to accept

- [x] **Turning the loop on loops what the clip holds now**, from its
      start to its end, so a 1-bar clip loops 1 bar. An assistant can
      give another length (`loop_beats`). The alternative is to always
      loop a whole number of bars.
- [x] **Each time round rolls afresh.** Notes with a chance, or a pick
      of one, are rolled again each time round the loop rather than
      playing the same way every time.
- [x] **Controller lanes loop with the notes** (a filter move in a
      4-bar loop happens every 4 bars). Rendering note effects into a
      looping clip writes its notes out in full and switches its loop
      off; its controller lanes then play once.
