# 0007 — UI toolkit

- **Status:** Accepted 2026-10-27 (egui), subject to the speed check on the owner's PC
- **Date:** 2026-10-24

## Context
The UI must handle large timelines at 60 fps, embed native plugin editor
windows, support text input/IME, menus, drag-and-drop and high-DPI, and
work with screen readers.

## Options (if Rust)
1. **Slint**: declarative, designer-friendly. Check its licence options
   against decision 0001.
2. **egui**: immediate mode, fast to build tools with. Check its styling and
   accessibility depth.
3. **iced**: Elm-style architecture.
4. **Vizia**: built with audio UIs in mind.
5. **Hybrid**: one of the above for panels and dialogs, plus a custom GPU
   canvas (`wgpu`) for the timeline, waveforms, meters and editors.

If decision 0003 falls back to C++, JUCE's own UI framework is the default.

## Evaluation criteria
Timeline benchmark, plugin-window embedding, accessibility (e.g. AccessKit
support), text/IME, theming for an original visual language, licence, and
maintenance activity.

## Spike results so far (2026-10-27)
- **egui** (first candidate, `spikes/ui-timeline`): a 320-clip timeline
  with waveforms, menus, shortcuts, drag with snap, a rename dialog and a
  screen-reader description. Building each frame took about 1 ms of our
  code. The 60 fps check needs a real GPU and waits for the owner's PC.
- egui bundles default fonts under the SIL Open Font Licence 1.1 and the
  Ubuntu Font Licence 1.0. The licence check allows these for that one
  crate only, as a stopgap. If egui is chosen, this decision must confirm
  the product can ship those fonts or replace them with fonts whose
  licences we have checked.
- **Slint** (second candidate, `spikes/ui-slint`): the same timeline,
  built 2026-10-27. Each clip is an element with its own screen-reader
  name and selected state; waveforms are images drawn in Rust. Menus,
  dialog, text entry, shortcuts and dragging all worked when driven by
  scripted input in the cloud. The real speed check waits for the owner's
  PC, as for egui.
- Slint's open-source licence is GPL-3.0-only. It can be combined with our
  GPL-3.0-or-later code, but the combined program could then only be
  distributed under GPLv3. The licence check allows it for the Slint crates
  only, pending this decision.
- **Plugin editor inside the window** (`spikes/embed-panel`, both
  toolkits): a native child window placed over an area of the toolkit's
  window and moved with its layout. On Linux (X11, in the cloud) it stayed
  visible and in place in both toolkits, including through a window
  resize. On Windows it hosts a real VST 3 editor (`--plugin`), which
  waits for the owner's PC.
- Not tried: iced and Vizia.

## Decision
**egui**, chosen by the owner on 2026-10-27. The reason given: it suits
agent-first development better. The whole UI is plain Rust code, with no
separate UI language or designer tool, so changes can be made, built and
checked in one place.

Follow-ups:
- The 60 fps check on the owner's PC still has to pass. If it fails
  badly, this decision is reopened.
- The bundled fonts (SIL Open Font Licence 1.1 and Ubuntu Font Licence
  1.0) are still allowed only as a stopgap. Confirm they can ship, or
  replace them.
- Each clip should be given its own screen-reader entry, which Slint gave
  for free. egui draws the timeline as one area today.
- The Slint spike was removed from the build after this decision. Its
  code is in the history at commit 0f8edaf.
