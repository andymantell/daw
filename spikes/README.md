# Phase 0 spikes

Throwaway prototypes for the Rust go/no-go spike
([ROADMAP](../docs/ROADMAP.md) 0.1, [decision 0003](../docs/decisions/0003-implementation-language.md)).
Nothing here ships. Licence: GPL-3.0-or-later.

| Test | Crate | Status |
|---|---|---|
| **A. Audio I/O** (ASIO on Windows) | `audio-io` | Built. Not yet run on the owner's PC and interface. |
| **B. VST 3 hosting** | `vst3-scan` (step one: list installed plugins) | Scanner built. Loading plugins not started. |
| **C. UI** | — | Not started. |
| **D. Engine** | `engine` | Passing (see below). |

Collaboration spike ([ROADMAP](../docs/ROADMAP.md) 0.5,
[decision 0004](../docs/decisions/0004-collaboration-model.md)):

| Part | Crate | Status |
|---|---|---|
| Project document on a CRDT | `collab-doc` | Both candidates pass. Loro preferred (see below). |
| Live peer-to-peer connection | — | Not started. Needs real home, mobile and school networks. |

Test machine: Windows only for now, so the macOS (CoreAudio) half of A
and B is still open.

## A. Audio I/O: `audio-io`

Opens one device for input and output at once (ASIO by default on
Windows), plays input 1 straight back out on outputs 1 and 2 so you can
hear the monitoring, and spends a fixed share of every callback on dummy
DSP work. It counts every sign of a dropout: callbacks slower than real
time, gaps between callbacks of more than 1.5 buffers, the output running
out of input, and stream errors. It also reports the driver's own latency
figures and any memory allocation on the audio threads.

On the test PC (from the folder holding `audio-io.exe`):

```
audio-io --list
audio-io --device UMC --buffer 128 --minutes 30 --load 0.5
```

The report is saved as `audio-spike-report.txt`. Pass criterion
(ROADMAP 0.1 A): 30 minutes at 64–128 frames and 48 kHz with no dropouts.

Checked so far only against Linux's null audio device in the cloud
container, which isn't paced in real time, so it shows the detector working
but says nothing about real drivers.

## D. Engine

`engine` has a lock-free command queue from the UI thread to the audio
thread, and swaps the whole render graph by moving a pointer through that
queue. The audio thread crossfades the old and new graphs over one block,
then hands the old graph back through a second queue so it is freed on the
UI thread.

`cargo test --release -p spike-engine` checks:

- **No allocation on the audio thread.** A global allocator counts every
  allocate, reallocate or free made while a thread is marked real-time.
  Over 30 minutes of audio (675,000 blocks of 128 samples at 48 kHz,
  rendered faster than real time), with the UI thread sending (in one run) about 11
  million gain changes and 1.6 million graph swaps, the count is zero.
- **Glitch-free swaps.** The output never steps further between samples
  than its oscillators naturally can. A control test shows that a hard swap
  *does* trip the same detector, and the crossfade removes it.
- **The detector works.** A deliberate allocation inside the real-time
  section is counted.

Limits: this runs without a sound card and faster than real time. Timing
under a real driver callback is part of test A.

## B, step one: `vst3-scan`

Lists every `.vst3` in the standard folders (on Windows,
`C:\Program Files\Common Files\VST3`) plus any folders passed as
arguments, and reads each bundle's `moduleinfo.json` for name, vendor,
version and category. It only reads files and never loads plugin code.
The report is saved as `vst3-scan-report.txt` in the current folder.

Download it, and `audio-io.exe`, from the latest **spikes** workflow run on
GitHub (artifact `spikes-windows-x64`).

## 0.5, first half: `collab-doc`

The same small project model (tracks with a name, gain and clips, in a
user-chosen order) is written against two CRDT libraries, Loro and yrs,
behind one trait. `cargo test --release -p spike-collab-doc` runs every
scenario on both:

- Teacher and student edit at the same time (including a conflicting
  rename): both copies end up identical, and every non-conflicting edit
  survives.
- One side deletes a track while the other edits it: the delete wins
  cleanly on both sides.
- Both sides move the same track at once: no duplicates, same order.
- 300 random edits on each side while offline, then one sync.
- Three peers making 600 random edits with random pairwise syncs, then
  meeting once.
- Per-user undo: the teacher undoes their own change after the student
  has made later edits; only the teacher's change is reverted.
- Saving and reloading gives back the same project.

All pass on both libraries.

`cargo run --release -p spike-collab-doc --example size_report`, for 200
tracks of 20 clips each followed by 10,000 edits (one run in the cloud
container; times are indicative only):

| Library | Saved size, new | Saved size after edits | One edit sent to a peer | Build | Per edit | Load + read |
|---|---|---|---|---|---|---|
| Loro | 219 KB | 243 KB | 96 B | 64 ms | 5.4 µs | 77 ms |
| yrs | 178 KB | 296 KB | 627 B | 21 ms | 4.1 µs | 35 ms |

Both are small and fast enough for a lesson project. **Loro is the
preferred candidate**, because:

- It has a movable list, so track and clip order is a real CRDT
  operation. yrs has no move, so the yrs version needs a hand-written
  fractional-position scheme, which is more code to get right and wears
  out after many moves into the same gap.
- Its history grows more slowly under edits (243 KB against 296 KB after
  10,000 edits), and a single edit costs far less to send (96 bytes
  against 627).
- Per-peer undo is built in. yrs needs an origin convention to keep
  remote changes out of the undo stack.

yrs loads faster and starts smaller, which is worth re-checking on a much
larger project before committing.
