# Phase 4 (B1): review mode, suggested edits

A teacher (or anyone) can suggest edits to a song without changing it;
the person the song belongs to accepts or rejects each change.

- [ ] **Suggesting.** Screenshot: `app-suggesting.png`. Edit → Suggest
      edits (or Ctrl+K "suggest"). Change the tempo, rename a track, move
      a clip, add a marker. Pass: a strip at the bottom right says
      "Suggesting edits: … The song isn't changed." Everything you see
      and hear is the suggested version, and Undo steps through only the
      suggested edits. A comment written meanwhile goes to the song
      straight away. Stop suggesting (on the strip, or Edit → Stop
      suggesting): the song is exactly as it was before, and Undo has
      nothing new on it.
- [ ] **Saved with the project.** Suggest some edits, save, close and
      reopen (or send a bundle to the second copy). Pass: the suggestion
      is still listed with all its changes; "Carry on suggesting" picks it
      up where it was left, edits so far included.
- [ ] **Reviewing.** Screenshot: `app-suggested-edits.png`. Edit →
      Suggested edits…. Pass: each suggestion is listed with who made it;
      choosing one lists its changes by place (Song, each track, Markers)
      in the same words as Snapshots → Changes…, each with Accept and
      Reject. Accept one: only that change reaches the song, it shows
      "Accepted", and Ctrl+Z takes it back as one step ("Accept
      suggestion: Tempo 120 → 96 BPM" on the undo list). Reject one: the
      song is untouched and it shows "Rejected". Accept all / Reject all
      deal with every change still waiting.
- [ ] **The teacher as a commenter.** Let the teacher in as a commenter
      only. Pass: they can suggest edits (and change only their own
      suggestions), their suggestion reaches your copy through the shared
      folder or a bundle, and only you (the owner or an editor) can
      accept it.
- [ ] **Changed since.** Suggest a tempo change, then change the tempo
      yourself in the song before reviewing. Pass: the change says
      "Changed in the song since: accepting replaces that".
- [ ] **Assistants.** `get_suggestions` lists suggestions and their
      changes by key; `edit_suggestion` starts, stops, resumes, accepts,
      rejects, renames and deletes. While an assistant has started
      suggesting, every edit tool it uses goes to the suggestion.

## Decisions to accept

- [x] **What counts as one change.** A song setting (tempo, time
      signature, key, loop, tuning reference, timecode, the order
      sections play in, the master channel, lesson notes); one marker,
      tempo change, pattern, tab block, group, video or similar item; a
      whole track added or removed; and for a track that both have, its
      settings (mix, effects, routing, name…) as one change and each clip
      and note clip as one change each. So a moved clip can be taken
      without the teacher's volume change on the same track, but two
      effect tweaks on one track come as one change.
- [x] **Comments and assignments are never suggested.** They go to the
      song while suggesting, as they always do.
- [x] **Commenters may suggest.** A teacher let in only to comment can
      make suggestions (their own only), but not accept them.

## Known gaps

- Accepting a change to a track's settings takes its whole settings as
  suggested, so an edit the student made to the same track's settings
  meanwhile is replaced (the change is marked "Changed in the song
  since" first).
- A track moved up or down the list on its own isn't a change.
- Practice and lessons written while suggesting go to the song, like
  comments; that's intended but untested on a device.
