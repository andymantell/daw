# Plugin compatibility

Which VST 3 plugins have been tried with this app, and how they did.

Each row comes from the app's own plugin check: Settings → Plugins →
**Check plugins**, or `daw --check-plugins report.md` from a command
line. The check loads each plugin in a process of its own (so one that
crashes is noted and the rest are still checked), plays a second of sound
through it in the usual block size and in odd ones, saves its settings
and loads them into a second copy, and, where the plugin has them,
switches on its sidechain input and its extra outputs.

The check is a first pass, not a guarantee: it cannot see whether a
plugin sounds right or whether its own window works. Problems found by
hand are noted in the last column.

## The project's test plugins

Checked in CI on Windows, Linux and macOS on every change to the
plugin host (`crates/plugins/tests/depth.rs`), including builds of the
test plugin made to crash or hang.

| Plugin | Loads | Plays | Settings kept | Sidechain | Extra outputs | Notes |
|---|---|---|---|---|---|---|
| DAW Test Gain | yes | yes | yes | – | – | |
| DAW Test Router | yes | yes | yes | yes | yes | Changes its delay while playing; stands in for an older version of itself |
| DAW Test Gain, built to crash while playing | yes | no: the plugin process crashed | – | – | – | The app carries on, as it should |

## Plugins on the owner's computer

Not checked yet. To be filled in from the first report made on the
owner's PC (see `docs/review-queue/2.4-plugin-depth.md`).

| Plugin | Vendor | Loads | Plays | Settings kept | Sidechain | Extra outputs | Notes |
|---|---|---|---|---|---|---|---|

## Adding a row

Run the check, then copy the rows from the report it writes into the
table above, with the date, the app version (in the report's first
line) and anything found by hand in Notes.
