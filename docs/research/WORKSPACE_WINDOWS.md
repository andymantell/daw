# Research: docked panes and detachable windows

Checked 2026-10-29, for a feature request: optionally detach panes into their
own windows so the app can spread across three screens, while keeping
in-window pane management as the default.

## Findings (UI toolkit chosen in decision 0007)

- **Multiple native windows are supported.** The toolkit's "viewports" open
  real OS windows on its native backend (not on the web). Viewports form a
  tree, and the root viewport can't close without closing the app.
  *(Source: the toolkit's API docs, `egui::viewport`, version 0.37.0, read
  2026-10-29.)*
  - *Deferred* viewports repaint on their own, so they are the efficient
    choice for long-lived windows. They share state through channels or
    locks.
  - *Immediate* viewports are simpler, but repaint with their parent, so
    CPU work grows with the number of windows. Use them only for short-lived
    windows.
  - Where real windows aren't available, the toolkit falls back to windows
    drawn inside the parent. Several viewport features are still listed as
    planned in the toolkit's tracking issue.
- **Docking inside the main window:** the most used open-source docking
  library for the toolkit (MIT) handles splits and tabs. Its "undock"
  makes floating windows *inside* the app window, not OS windows. Its
  current release targets toolkit 0.36, one version behind ours. *(Source:
  its docs.rs page, version 0.21.1, read 2026-10-29.)*
- **Consequence:** in-window docking can come from that library or from our
  own small layout tree. Tearing a pane off onto another screen is our own
  work: put the pane in a deferred viewport, and dock it back on close or
  drop. Either way, the pane layout should be our own data, so it can be
  saved, restored and checked by tests.

## Design notes

- Panes are drawn by the same code whether they are docked or detached. A
  pane never assumes which window it is in.
- Window layouts are local settings (per computer). The project document
  stores only which panes are open, so collaborators aren't affected.
- Plugin editor windows are native child windows (see the embedding spike).
  Moving a plugin pane into a detached window must re-parent the editor to
  that window. This needs testing per plugin format on each OS.

## Not researched yet

- Re-parenting a running plugin editor between windows on Windows, macOS
  and Linux.
- Restoring windows when the set of screens changes, on each OS.
- Whether the docking library's next release tracks our toolkit version.
