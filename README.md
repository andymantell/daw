# DAW (working title)

A **native desktop digital audio workstation** for Windows, macOS and Linux.

It starts as a tool for **guitar lessons**: teacher and student share a
project, record takes through their amp-sim plugins with low latency,
practise against backing tracks at reduced speed, and leave feedback pinned
to the music. From there it grows into a full **professional DAW** with
first-class **VST 3** support and a **Live Looper** performance mode in
which every loop and layer is visible on the timeline.

There is **no backend server**. Projects are folders on your disk, and
collaboration is peer-to-peer (the teacher hosts a session) or via project
bundles and shared folders.

> **Status:** Phase 0. Proceeding with Rust at risk while the on-device
> spike tests wait for the test machine (see
> [decision 0003](docs/decisions/0003-implementation-language.md)). Steps
> waiting for the owner are listed in the
> [review queue](docs/review-queue/README.md).

## Screenshots

A snapshot of the app as it stands, with a deep house test song built
through the assistant interface (122 bpm, A minor; the vocal is a
synthesised stand-in). These will change as the app does.

**Arrangement:** song sections and linked clip copies.

![The arrangement of a deep house song](docs/screenshots/arrangement.png)

**Mixer:** desk-style strips with vertical faders, meters, routing and
sends, playing the drop.

![The mixer as a desk of vertical channel strips](docs/screenshots/mixer.png)

**Note editor:** the electric piano part, with the key, quantise, groove
and velocity lane.

![The note editor showing a chord part](docs/screenshots/note-editor.png)

**Pitch editor:** a vocal phrase split into notes, with its vibrato
and drift.

![The pitch editor showing a vocal phrase](docs/screenshots/pitch-editor.png)

**Synth:** the three-oscillator mono synth playing the bass line.

![A built-in synth's window over the arrangement](docs/screenshots/synth.png)

## A demo project

`examples/house-demo` is a layered house track (124 bpm, A minor) that
shows the app off: drum machine patterns, every built-in synth, a sung
phrase (a synthesised stand-in) with its pitch found, buses, sends,
effects, automation, sections, a loop and comments. Open it with File →
Open project… and pick the folder. It was built entirely through the
assistant interface by `crates/app/src/demo_song.rs`, which doubles as
a test that an assistant can make a whole song; running that test's
`write_house_demo` rebuilds the folder.

## Running it on Windows

**Ready-made build:** every push to `main` builds `daw.exe` in CI. Open
the latest green **ci** run on `main` under the repository's Actions tab
and download the **daw-windows-x64** artifact (a zip holding `daw.exe`).

**From source** (what the Windows CI job does):

1. Install the Visual Studio Build Tools with the **Desktop development
   with C++** workload (the MSVC linker Rust needs).
2. Install Rust with [rustup](https://rustup.rs), stable toolchain
   (1.89 or newer).
3. Install LLVM and point `LIBCLANG_PATH` at it, for example
   `C:\Program Files\LLVM\bin`. The ASIO bindings are generated with
   bindgen, which needs libclang.
4. From the repository folder, run:

   ```
   cargo run --release -p daw-app
   ```

   Use a release build for anything with audio. The first build
   downloads Steinberg's ASIO SDK, so it needs internet access.

For the lowest delay, install your audio interface maker's ASIO driver.
The first time the app starts with no audio settings saved, the guided
audio setup opens and offers the ASIO device first.

## Running it on a Mac

Releases carry `daw-<version>-macos-universal.dmg`, one app for Apple
Silicon and Intel Macs on macOS 13 or later. The release workflow can
also be run by hand from the Actions tab; its **release-files** artifact
holds the same .dmg. Open the .dmg and drag DAW onto Applications.

The app is not signed by a registered developer, so macOS refuses it
the first time:

1. Open DAW from Applications. macOS says it can't open the app; click
   **Done**.
2. Open **System Settings**, click **Privacy & Security**, scroll down,
   and click **Open Anyway** next to the note about DAW.
3. Click **Open** in the warning that follows (macOS may ask for your
   password). From then on DAW opens normally.

Allow the microphone when DAW asks, or recordings stay silent. To change
it later: System Settings → Privacy & Security → Microphone.

No Mac has tested these builds yet; they are built and checked in CI
only.

## Documents

| Document | Purpose |
|---|---|
| [docs/ROADMAP.md](docs/ROADMAP.md) | Phased plan: Rust spike, Lesson Room vertical slice, then a full DAW |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Proposed architecture: engine, plugin hosting, project format, P2P collaboration |
| [docs/CLEAN_ROOM.md](docs/CLEAN_ROOM.md) | Rules for keeping the project original and legally defensible |
| [docs/GLOSSARY.md](docs/GLOSSARY.md) | Project vocabulary (generic and original terms only) |
| [docs/research/CAPABILITY_LANDSCAPE.md](docs/research/CAPABILITY_LANDSCAPE.md) | Generic survey of DAW and loop-station capabilities, mapped to phases, plus research gaps |
| [docs/decisions/](docs/decisions/) | Decision records |
| [CONTRIBUTING.md](CONTRIBUTING.md) | How to contribute: clean-room rules, sign-off, checks to run |
| [spikes/](spikes/) | Phase 0 throwaway prototypes and their results |

## Principles

- **Native, low-latency audio:** ASIO, CoreAudio, WASAPI, ALSA, JACK, PipeWire.
- **Plugins first-class:** VST 3 from the first release; CLAP, AU and ARA after.
- **Local-first and serverless:** your data stays on your machines.
- **Original work:** clean-room policy; no copying or reverse engineering.
- **Audio correctness:** sample-accurate timing, no dropouts, automated audio tests.

## Licence

GNU General Public License v3.0 or later (`GPL-3.0-or-later`). See
[LICENSE](LICENSE) and [decision 0001](docs/decisions/0001-project-licence.md).
