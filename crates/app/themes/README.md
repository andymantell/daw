# Theme files

Each `.theme` file here is one of the app's built-in looks (roadmap
2.10). Your own themes use the same format and go in a `themes` folder
inside the settings folder (`%APPDATA%\DAW\themes` on Windows,
`~/.config/DAW/themes` elsewhere). Settings → Theme → *Copy to edit*
makes one for you to start from, and *Reload* picks up your changes.

## Format

Plain text: `name = value` lines under `[section]` headings. A `#` at the
start of a line, or after a space (and not part of a colour), starts a
comment.

A file of your own only needs what it changes: `based_on = <id>` (the
file name of a built-in theme, such as `vellum`) gives it everything else
from that theme, Valve Glow if left out.

| Key | Meaning |
|---|---|
| `name`, `about` | Shown in Settings → Theme |
| `designed` | `dark` or `light`: the colours shown when the choice is *As designed* |
| `[shape] radius`, `clip_radius` | Corner radius of controls and of clips, in points |
| `[shape] line` | Weight of borders and lines |
| `[shape] gap`, `padding` | Space between controls and inside buttons, across then down |
| `[shape] shadow` | Blur of the shadow under clips and windows; `0` for none |
| `[shape] loop` | The loop range on the ruler: `filled` or `hatched` |
| `[type] body`, `small`, `heading`, `numerals` | Text sizes |
| `[type] font`, `numerals_font` | Typeface for text and for readouts: `barlow-condensed`, `ibm-plex-mono`, `ibm-plex-sans-condensed`, `libre-franklin`, `nunito`, or egui's own `default` and `default-mono` |
| `[waveform] style` | `filled`, `outline`, `line` or `trace` (a line with a glow) |
| `[waveform] weight` | Line weight of outlined and line waveforms |
| `[texture] kind` | `none` or `grain` (paper specks over the timeline) |
| `[glow] on`, `radius` | A glow around the playhead and traced waveforms |
| `[dark]`, `[light]` | The two sets of colours, below |

Colours are `#rrggbb`, or `#rrggbbaa` to be see-through. The clip colours
can also be `track` (the track's colour) or `track 35%` (see-through).

| Colour | Used for |
|---|---|
| `bg`, `panel`, `lane`, `raised` | Behind everything; panels, the ruler and track names; the timeline's lanes; buttons, fields and mixer strips |
| `line`, `grid`, `grid_fine` | Borders; bar lines; lines between beats |
| `text`, `text_dim` | Text, and quieter text |
| `accent`, `on_accent` | Hovered controls, the loop's ends, focus rings; text on markers |
| `selection` | Behind selected text and switched-on buttons |
| `record`, `playhead`, `glow`, `loop`, `marker`, `comment` | Live state on the timeline |
| `tempo`, `signature` | Tempo and time-signature changes on the ruler (labelled in `bg`) |
| `clip_fill`, `clip_border`, `clip_text`, `clip_label` | Clips; `clip_label` is a label behind the name, or `none` |
| `meter_bg`, `meter_low`, `meter_mid`, `meter_high` | Meters: below −12 dB, up to 0 dB, over |
| `good`, `warn`, `error` | Messages |
| `texture`, `shadow` | The grain's specks, and shadows |
| `tracks` | Track colours, used in turn, separated by spaces |

## Fonts

The typefaces are in `crates/app/fonts/`, each with its licence (SIL Open
Font License 1.1) beside it. They are the unmodified files from the
Google Fonts repository (github.com/google/fonts, `ofl/` folder, commit
6157dfa); only the file names changed. Libre Franklin and Nunito are
variable fonts, drawn at weights 400 and 500. The OFL lets fonts be
bundled with any software, including GPL software, provided they are not
sold on their own and changed versions don't use the reserved names
("Plex" for IBM Plex).

## Contrast

`cargo test` checks every built-in theme in both variants against WCAG AA:
4.5:1 for text and 3:1 for marks that carry meaning (the playhead, record,
waveforms, meters). A theme that fails doesn't build. Live state never
relies on colour alone: the playhead has a head on the ruler, the loop
range has marked ends, and comment pins and markers have their own shapes.
