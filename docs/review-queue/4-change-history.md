# Phase 4 (A10): change history and the undo list

- [ ] **Undo list.** Screenshot: `app-undo-list.png`. Edit → Undo
      list… (or search "undo list" with Ctrl+K). Make five or six edits:
      add a track, change the tempo, add two markers, turn a volume
      down. Pass: each edit is listed by what it changed ("Tempo 120 →
      96 BPM", "Markers: Added “Solo” at bar 9"), newest at the top,
      with how long ago. Hovering a step lists everything it changed.
      Choose a step three down: the project goes back to just after it
      in one go, and the steps above it turn grey above "Now". Choose a
      grey one: the project goes forward to it. "Before these changes"
      undoes them all. Ctrl+Z and Ctrl+Y still go one step at a time,
      and a new edit after going back drops the grey steps.
- [ ] **Activity.** Screenshot: `app-activity.png`. The Activity tab in
      the same window (or Edit → Activity…). Share a project with the
      teacher (or a second copy signed in under another name), let them
      in as an editor, and have them change something. Pass: their
      change shows under their name, yours under yours marked "(you)",
      newest first, with when (hover for the time in UTC). A person's
      changes made within ten minutes of each other are one entry.
      Signing in, practice settings and taking snapshots aren't listed,
      as they don't change the song. Show more reaches further back.
- [ ] **Assistants.** The agent interface has `get_history` (the undo
      and redo lists and the activity) and `undo_steps` (`{"undo": 3}`
      or `{"redo": 1}`); an assistant's own request shows on the undo
      list under the name it gave.

## Decisions to accept

- [ ] **Only your own steps are on the undo list.** Undo never takes
      back someone else's change (as before); their changes are on the
      Activity tab, not the undo list.
- [ ] **Times start now.** Changes are timed from this version on; an
      older project's history shows "time not recorded" for what came
      before. Who made a change is known for anything since that
      person's copy first signed in; older changes say "Someone".

## Known gaps

- Comments and assignments aren't described in the history (the same
  comparison as Snapshots → Changes… is used, which leaves them out), so
  a step that only writes a comment reads "An edit".
- The arrow in "120 → 96" draws as a box in the test screenshots, as it
  does in the Snapshots ones; check it shows on Windows.
