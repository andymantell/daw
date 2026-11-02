# UI polish, step two: one top bar

Step two of the UI polish plan (`docs/plans/ui-polish.md`, sections 1
and 2). The menu bar, the status line and the four transport rows are
now one row. Nothing was removed. Each control moved into a panel that
opens from the button it belongs to, and every command keeps its id and
keys.

## What changed

![The top bar at 1920 x 1200](ui-polish-2-top-bar/1920-main.png)

![At 1440 x 900](ui-polish-2-top-bar/1440-main.png)

From the left:

- **The menus**, as in step one.
- **The transport**: back to the start, Play, Record, Loop and the
  metronome. Record, Loop and the metronome each have a small arrow
  that opens their options.
- **The position** in bars and in minutes and seconds (or timecode, if
  you chose to show it).
- **Tempo** (drag it, or double-click to type), then **time signature**
  and **key** as buttons that open small editors.
- **Speed**, which shows the practice speed. It lights up while the
  speed or pitch is changed, so you can't forget you're playing slowed
  down.
- On the right: **Find a command** (Ctrl+K), **one status pill**, and
  **AI assistants**.

### The option panels

| Was always on screen | Now |
|---|---|
| Punch bars, pre-roll and post-roll, always keep, Keep last, notes recording mode, Keep notes | Record options (arrow by Record) |
| Clicks per beat, click level, count-in | Metronome options (arrow by the metronome), plus the existing "higher click on the first beat" setting |
| Loop bars from and to | Loop options (arrow by Loop), or **drag the loop's ends along the foot of the ruler** |
| Time signature, Changes… | The time signature button: beats per bar, the beat, and "Tempo and signature changes…" |
| Key (only in the note editor before) | The key button: root, scale, or no key |
| Speed and presets, transpose and cents, tuning presets, detect tuning, the speed trainer | The Speed button |
| Device, rate, DSP load, "No audio device", busy work, recording or disk problems | The status pill shows the most important line; click it for all of them, with Audio settings… and Performance… |

![Record options](ui-polish-2-top-bar/1920-record.png)
![Metronome options](ui-polish-2-top-bar/1920-metronome.png)
![Loop options](ui-polish-2-top-bar/1920-loop.png)
![Time signature](ui-polish-2-top-bar/1920-time-signature.png)
![Speed and pitch](ui-polish-2-top-bar/1440-speed.png)
![Key](ui-polish-2-top-bar/1440-key.png)
![Status](ui-polish-2-top-bar/1440-status.png)

The panels use switches instead of drop-downs, as the plan's panel kit
suggests. Clicking elsewhere or pressing Escape closes them.

**Slowed down**, the Speed button lights up:

![Speed 75%](ui-polish-2-top-bar/1440-slowed-bar.png)

**Simpler mode**: bigger buttons, Loop and Click are labelled, and the
time signature and key are hidden. Speed stays.

![Simpler mode, 1440](ui-polish-2-top-bar/1440-simple-bar.png)
![Simpler mode, 1920](ui-polish-2-top-bar/1920-simple-bar.png)

## For assistants

- New commands: `record-options`, `loop-options`, `metronome-options`
  and `speed-options` open a panel for the person to see. `keep-last`
  makes takes of what armed tracks played without Record on (before
  this, only a button did that).
- New tool `set_record_options`: punch in and out, pre-roll,
  post-roll, how much input is kept, and how notes are recorded.
  These controls had no assistant route before. `get_transport` now
  reads them back as `record_options`.
- `set_practice` can now set the metronome's `accent`.
- `get_selection` says which panel is open, as `top_bar`.
- End-to-end tests: `top_bar::tests` (driven through the assistant
  interface, plus dragging the loop's end on the ruler).

## Decisions for Andy

1. **Count-in sits in the metronome options**, not the record options
   that the mockup showed. It counts in before plain playback too, and
   it is clicks. It's also still in the Song menu and on the C key.
2. **Speed, transpose and the trainer are in a Speed button's panel
   for now.** The plan moves them to the Practice panel in step seven.
   Until then, this keeps them one click away, and the lit button warns
   you when they're changed.
3. **Tempo stays a number you drag or type**, rather than a button that
   opens an editor. Tempo changes later in the song open from the time
   signature panel.
4. **The loop has its own options arrow** as well as dragging on the
   ruler, so the bars can still be typed and reached from the keyboard.
5. **Keyboard shortcuts pause while a panel is open**, as they do while
   a menu is open. For example, Space doesn't play until the panel is
   closed.
6. **"Recording at 75%" now shows in the status pill.** Before, it was
   a line on the transport row. When "No audio device" is also showing,
   the pill shows that first and the slowed-down warning is in its list.
