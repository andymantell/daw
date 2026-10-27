# 0007 — UI toolkit

- **Status:** Open (decided by spike 0.1C)
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
- Not yet tried: iced and Vizia, and embedding a plugin editor inside the
  toolkit's window (spike B opens editors in a separate window).

## Decision
_Not yet made._
