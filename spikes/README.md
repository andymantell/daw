# Phase 0 spikes

Throwaway prototypes for the Rust go/no-go spike
([ROADMAP](../docs/ROADMAP.md) 0.1, [decision 0003](../docs/decisions/0003-implementation-language.md)).
Nothing here ships. Licence: GPL-3.0-or-later.

| Test | Crate | Status |
|---|---|---|
| **A. Audio I/O** (ASIO on Windows) | `audio-io` | Built. Not yet run on the owner's PC and interface. |
| **B. VST 3 hosting** | `vst3-scan`, `plugin-host`, `vst3-test-plugin` | Host passes its automated tests with our own test plugin on Linux and Windows CI. Not yet run against real plugins on the owner's PC. |
| **C. UI** | `ui-timeline` | First toolkit (egui) built for Windows. Its own code takes about 1 ms a frame; the 60 fps check still needs a real GPU, so it waits for the owner's PC. |
| **D. Engine** | `engine` | Passing (see below). |

Collaboration spike ([ROADMAP](../docs/ROADMAP.md) 0.5,
[decision 0004](../docs/decisions/0004-collaboration-model.md)):

| Part | Crate | Status |
|---|---|---|
| Project document on a CRDT | `collab-doc` | Both candidates pass. Loro preferred (see below). |
| Live peer-to-peer connection | — | Not started. Needs real home, mobile and school networks. |

Latency spike ([ROADMAP](../docs/ROADMAP.md) 0.4): `latency`. Built;
needs a loopback cable on the owner's interface.

Speed and pitch spike ([ROADMAP](../docs/ROADMAP.md) 0.6): `stretch`.
CPU and objective checks done; listening tests still to do (see below).

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

## C. UI: `ui-timeline`

First candidate toolkit: egui (through eframe, drawing with the GPU). The
test is a timeline of 32 tracks with 10 clips each (320 clips), every clip
showing a waveform drawn from a multi-resolution peak summary, one column
per pixel. It covers the everyday pieces a DAW needs: a menu bar, keyboard
shortcuts (Space plays, F2 renames, Delete removes, +/- zoom), wheel scroll,
Ctrl+wheel zoom around the mouse, dragging clips with a 0.25 s snap, a
rename dialog with text entry, and a screen-reader description of the
timeline (egui exposes this through AccessKit).

```
ui-timeline                         interactive
ui-timeline --bench 20              scrolls and zooms by itself for 20 s,
                                    writes ui-bench-report.txt, then exits
ui-timeline --tracks 64 --clips-per-track 20   heavier session
```

Pass mark: 99% of frames under 16.7 ms of CPU and at least 55 fps on
average, at 1280×800, on the owner's PC.

Result in a cloud container with no GPU (software drawing on 4 cores,
10 s run): our own code that builds each frame took a median of 1.2 ms
(99th percentile 2.6 ms). The whole frame, including software drawing,
took a median of 9.7 ms but reached 69 ms at the 99th percentile, and
averaged 22 fps. That run says the timeline logic is cheap. It says
nothing about real drawing speed, so the verdict waits for a run on the
owner's PC. Not yet checked: a screen reader actually reading the
timeline, high-DPI scaling, and input method editors for typing.

## 0.4: round-trip latency, `latency`

Measures the true round trip with a cable from an output straight into an
input, and compares it with what the driver reports. It plays six short
swept-sine pulses and finds each one in the recording by cross-correlation,
to a fraction of a sample. Then it stops and restarts the audio several
times per buffer size, because a recording offset is only usable if it is
the same every time audio starts.

```
latency --list
latency --device UMC --buffers 64,128,256 --repeats 5 --out 1 --in 1
```

For each run it reports:

- **Round trip:** where the pulse arrived in the input stream minus where
  it was sent in the output stream, counting frames from stream start.
- **Driver reports:** output plus input latency from the driver's
  timestamps.
- **Timestamp error:** the arrival by the driver's input timestamps minus
  the send time by its output timestamps. Zero means the reported
  latencies are honest. Anything else is the extra recording offset a DAW
  must apply.

Then, across restarts, a verdict: stable (within one frame), within one
block (meets the roadmap's ±1 block target), or unstable.

Tested so far only with unit tests of the pulse finder: whole and
fractional delays, noise, and polarity inversion. The stream side needs
real hardware. The second half of 0.4, recording a guitar DI against a
click, follows once the offsets are known.

## 0.6: speed and pitch, `stretch`

Real-time pitch-preserving speed change and transpose, behind one
interface that a track would call each audio block. Candidates:

| Candidate | What it is | Licence | Speed | Pitch |
|---|---|---|---|---|
| `signalsmith` | Signalsmith Stretch (C++) through its Rust wrapper | MIT | yes | yes |
| `timestretch` | Pure-Rust library, its wide-range keylock engine | MIT | yes | offline only, so not tested |
| `own-2048`, `own-4096` | Our phase vocoder (`src/pv.rs`), 2048- or 4096-sample FFT, written from the papers cited at the top of the file | ours | yes | yes |

```
stretch-bench                          synthetic guitar material
stretch-bench --input my-playing.wav   a real recording (48 kHz)
stretch-bench --seconds 30             longer CPU runs
```

It writes `stretch-report.txt` and WAV files for every candidate and
setting into `stretch-listen/`. CI runs it on Linux and uploads both as the
`stretch-listening` artifact.

Results in a cloud container (4 cores, no audio hardware, 48 kHz mono,
256-frame blocks, 10 s per setting; timings on shared machines are noisy):

| Candidate | Share of one core per track | Tracks per core | Pitch error | Attack smear at 50% speed |
|---|---|---|---|---|
| signalsmith | 1.6–2.6% | 39–61 | up to 2.5 cents | 62 ms |
| timestretch (speed only) | 3.6–15% (15% at 150% speed) | 7–28 | 0 | 40 ms |
| own-2048 | 1.1–5.1% | 20–88 | 0 | 37 ms |
| own-4096 | 1.9–6.0% | 17–53 | 0 | 49 ms |

- None of them allocated memory on the audio thread at any setting.
- All kept time: input used per output sample matched the speed to within
  0.1% (our vocoder's figure moves by up to one analysis hop because it
  works in whole frames).
- Pitch error is measured on a steady 220 Hz tone. 2.5 cents is under the
  5 cents often given as the smallest audible pitch difference.
- Attack smear is the time from 1% level to the peak of a single pluck
  (1 ms in the original). Every candidate softens pick attacks when slowed
  down. Ours resets phase at detected onsets, but the attack still spreads
  ahead of itself. Better transient handling is the obvious next step if
  we keep our own.
- Worst-case block times (up to a few ms) come from the shared machine
  and from frame-based processing doing a whole FFT frame in one block.
  On the owner's PC this needs checking alongside spike A.

So far: CPU is no obstacle for any of them, and both signalsmith and our
own vocoder cover the full 50–150% and ±12 semitone range. The choice now
rests on how they sound on real guitar. Still to do: listening tests on
real guitar recordings (`--input`), checking speed and pitch changes made
while playing, and stereo.

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

## B. VST 3 hosting: `plugin-host`

Written directly on the `vst3` Rust bindings (MIT/Apache-2.0), which is
how the product's plugin host would be built. For each plugin it:

1. loads the module and its factory, creates the processor, and creates and
   connects a separate controller when the plugin has one;
2. processes about a second of a 440 Hz tone and checks the output is
   finite;
3. reads the reported latency;
4. sweeps the first automatable parameter through the processor's
   parameter queue;
5. saves the state, restores it into two fresh instances and checks they
   then produce identical output;
6. on Windows with `--editor`, opens the plugin's editor embedded in a
   window we create, resizes it if the plugin allows, and closes it.

`check-all` runs each plugin in its own child process, so a plugin that
crashes or hangs (two-minute limit) is recorded as a failure instead of
ending the run.

`vst3-test-plugin` is our own small VST 3 effect (stereo gain, one
automatable parameter, 64 samples of real and reported latency, 8-byte
state), written from the public interface definitions. It lets CI test
the host on every platform without third-party plugins:
`cargo test --release -p plugin-host` checks the output sample for sample,
including the latency delay, a mid-stream gain change and a state
restore.

On the test PC:

```
plugin-host check-all --editor
```

The report is saved as `plugin-host-report.txt`.

Not covered yet: instrument plugins (they need note input), sidechain
buses, and running a plugin's audio processing in a separate process
during playback (the roadmap asks for that to be prototyped for at least
two plugins).

## B, step one: `vst3-scan`

Lists every `.vst3` in the standard folders (on Windows,
`C:\Program Files\Common Files\VST3`) plus any folders passed as
arguments, and reads each bundle's `moduleinfo.json` for name, vendor,
version and category. It only reads files and never loads plugin code.
The report is saved as `vst3-scan-report.txt` in the current folder.

Download it, with `audio-io.exe` and `plugin-host.exe`, from the latest
**spikes** workflow run on GitHub (artifact `spikes-windows-x64`).

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
