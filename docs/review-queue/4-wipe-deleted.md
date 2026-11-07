# Wiping deleted lesson words and takes

Built 2026-11-06, as the owner chose on 2026-11-05: deleted or
rewritten words in the lesson notes and tab blocks, and the audio of
deleted takes, really leave the project file, its backup and bundles.

How it behaves:

- Words deleted from the lesson notes, and deleted or rewritten tab
  blocks, are blanked out of the saved file's history, as deleted
  comments already were. The backup is replaced on that save, so it
  doesn't keep them either. While the project is open, Undo still
  brings them back.
- A snapshot taken before a deletion restores without the deleted
  words: the notes come back without the letters deleted since, and tab
  blocks only if they are still there, as they are now.
- A deleted take's audio file leaves the project's Audio folder once
  nothing uses it: not the project, not a snapshot, not a suggestion, and
  not another computer's copy in a shared folder. Undo lasts until the
  project is closed, so audio that only Undo could bring back stays
  until then (opening another project, a new one, or quitting). Audio
  this copy has never heard of (a take from another computer whose
  changes haven't arrived yet) is always left alone.
- File → Remove unused audio saves and removes it now, and says how
  much went and how much waits for Undo.

Known limit: takes recorded and deleted before a project is first saved
stay in the Unsaved takes folder; only project folders are cleaned.

## Checks on the PC

- [ ] **Deleted take leaves the folder.** Record two takes, save, delete
      one and save again. Open the project's Audio folder: both files
      are there (Undo could bring the take back). File → Remove unused
      audio says one file stays until the project is closed. Open
      another project, then look again. Pass: one file is gone, and the
      first project opens again with the remaining take playing.
