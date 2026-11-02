# UI polish, step three: the dock and the side panels

Step three of the UI polish plan (`docs/plans/ui-polish.md`, section 1,
"One dock at the bottom"). The instrument, effects and notation windows
becoming dock tabs is a thread of its own; this step makes the regions
they go into.

## What changed

**The main window has fixed regions now**, in place of panes split any
way:

- the **timeline** in the middle, always;
- the **left panel**, the whole height, where the content browser opens;
- the **right panel**, the whole height, where the inspector, lesson
  notes, comments, lessons and tuner open, as tabs;
- the **dock** along the bottom, under the timeline only, where the
  mixer and every editor open as tabs: note editor, pitch editor,
  automation, pattern editor, take lanes, monitor section, video and
  fretboard.

Each region shows its tabs in a header with the same buttons as before
(close, own window, full size, collapse, and a menu). A region with
nothing in it takes no room. Drag the edge between a region and the
timeline to resize it; collapse it to its header (a side panel to a
narrow strip) from its header button.

**Moving a pane**: drag its tab onto another region to make it a tab
there, or onto the timeline's left or right edge for that side panel
(anywhere else on the timeline for the dock). The pane's menu has
"Move it" with the other two regions. A pane can no longer sit beside
another inside a region; two panes in one region are tabs.

**Opening an editor shows its tab** in the dock, so the timeline keeps
its height. Double-clicking a note clip opens it in the note editor, as
before; double-clicking an audio clip now opens it in the pitch editor.

**Saved layouts carry over.** Layouts written before (in `layout.txt`,
the current one and the nine saved) are read once more: each pane goes
to its region, the tab that was shown is still shown, and the dock
keeps the height the timeline gave the panes below it. The built-in
Recording, Mixing and Lesson layouts are the same ideas in the new
regions (Lesson: notes, comments and tuner as tabs on the right).

**Fixed on the way**: a pane asked for from inside another pane (Edit
notes in the inspector, double-clicking a note clip, editing a pattern)
was lost, because the workspace was out being drawn. Those now open
once the panes are drawn.

**Sizes**: the left panel starts at a fifth of the width, the right at
about a sixth, the dock at 42% of the height and never less than 260
points while there's room, so an instrument panel fits at 1440 × 900.

## For the AI assistants

- New tool `set_pane`: opens a pane (its tab shown), moves it (`to`:
  left, right or dock), collapses or opens out its region, or closes it.
- `get_selection` has `panels`: for each region its panes, the one
  shown and whether it is collapsed; the panes in their own windows; the
  pane shown full size.
- The workspace's controls carry `// ai:` marks; `workspace.rs` has no
  unmarked controls left.
- End-to-end test: `workspace::tests::in_the_app::the_panels_work_with_an_assistant`.

## Pictures

In `ui-polish-3-dock-panels/`, at 1920 × 1200 and 1440 × 900 (the
song-3 test project):

- `before-*-main.png`, `before-*-notes.png`: before, with the note
  editor stacked under the timeline and mixer.
- `*-main.png`: the inspector on the right, the mixer in the dock.
- `*-browser.png`: the browser in the left panel.
- `*-notes.png`: the note editor as a dock tab beside the mixer and
  automation.
- `*-pitch.png`: the vocal in the pitch editor.
- `*-dock-collapsed.png`: the dock collapsed to its tabs.
- `*-lesson.png`: Layout 3, Lesson.

Made with `ui-polish/readme_shots_with_panels.rs` in the project files
(test `panels_review`), on top of the readme-shots patch.

## Known limits

- Many tabs in one region are clipped at its right edge rather than
  scrolled.
- The Practice tab of the right panel comes with step seven.
