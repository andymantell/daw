# Research: play-along video, stem separation and re-pitching

Checked 2026-10-27, for a feature request: video from online video platforms
as a first-class clip for playing along and recreating songs, stem
separation on any audio (including that video), and re-pitching (for
example, a recording tuned a half step down moved up to standard tuning).

## Findings

### Video from online platforms

- The largest platform's developer policies forbid, without its prior
  written approval, downloading, caching or storing its audiovisual content
  and making it available offline. They also forbid separating, isolating or
  modifying the audio or video components of its content, modifying or
  blocking any part of its player, and playing its content from a player
  that is not visible on screen. *(Source: the platform's API Services
  Developer Policies, sections III.E.1 and III.I.6–9, read 2026-10-27.)*
- Its embedded player API offers play, pause, seek and a playback rate
  chosen from the rates the player says it supports. The revision history
  mentions no pitch control and no access to the audio. The player runs as
  a web page from another origin, so the app cannot read its audio anyway.
- **Consequence:** an online video can be a *reference* clip that plays in
  the platform's own visible player, kept in sync with our transport. Stem
  separation, re-pitching, routing through the mixer and pitch-preserving
  slow-down are **not** allowed on it, and fetching the file with a
  third-party downloader would break the platform's terms. We will not ship
  that.
- **Supported route for full features:** video or audio *files* the user
  has the right to use (their own recordings, purchased downloads, lesson
  material). The audio of these goes through the engine like any other clip.
- Embedding a web player inside a native app needs an embedded web view.
  This depends on the UI toolkit choice (decision 0007).

### Stem separation

- Already planned for Phase 7, gated on model licences.
- The most widely used open-source separation code is MIT-licensed, but its
  README gives no licence for the pretrained weights. Its models were
  trained on a public benchmark dataset plus undisclosed "internal extra
  training data". *(Source: the project's README, read through a public
  mirror on 2026-10-27; we did not reach the original repository.)*
- That benchmark dataset is licensed "for educational purposes only" and
  bars commercial use without the copyright holders' permission. Some of its
  tracks are under non-commercial share-alike Creative Commons licences.
  *(Source: the dataset's record on its research-data archive, read
  2026-10-27.)*
- **Consequence:** the existing rule stands. Ship only models whose weights
  *and* training data allow our use. That may mean training our own model on
  licensed material, or letting users load a model they choose. Which
  route to take is still open.

### Re-pitching to another tuning

- Phase 1 already plans transpose by ±12 semitones (spike 0.6). A half step
  down to standard is +1 semitone.
- Two things are missing: **fine pitch in cents**, for recordings that are a
  little off concert pitch, and a **detected tuning offset**, so the app can
  suggest the shift.

## Not researched yet

- The minimum player size and branding rules for the embedded player. These
  are in the platform's separate "required minimum functionality" document,
  which we have not read yet.
- Whether other video platforms' terms differ.
- How closely an embedded player's position can be kept in sync with the
  audio engine. A spike will measure this.
