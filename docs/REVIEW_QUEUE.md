# Review queue

Steps that need the project owner: tests on the owner's PC and interface,
listening, and decisions to accept. Work carries on without them, at risk,
and every new item is added here as it comes up. Tick items off as they
are done, and record results in the linked documents.

Test machine: Windows PC with a Behringer UMC1820 interface.

## Getting the test programs

1. Open the repository's **Actions** tab, then the latest green **spikes**
   run on `main`.
2. Under **Artifacts**, download `spikes-windows-x64` (the Windows
   programs) and `stretch-listening` (audio files and report).
3. Unzip into a folder and open a terminal there. Each program writes a
   `*-report.txt` file next to itself. Commit the reports, or paste them
   into a chat thread.

## On the PC

Spike 0.1 decides whether Rust is confirmed
([decision 0003](decisions/0003-implementation-language.md)).

- [ ] **A. Audio, 30-minute run.** List devices, then run at 128 and at 64
      frames:
      `audio-io --list`
      `audio-io --device UMC --buffer 128 --minutes 30 --load 0.5`
      `audio-io --device UMC --buffer 64 --minutes 30 --load 0.5`
      Pass: no overruns, underruns or late callbacks after warm-up.
      Details: [spikes/README.md](../spikes/README.md#a-audio-io-audio-io).
- [ ] **B. Plugin scan.** `vst3-scan` lists what is installed. Then we pick
      10–15 plugins (two amp simulators, one copy-protected, some
      instruments) and fill gaps with free ones.
- [ ] **B. Plugin checks.** `plugin-host check-all --editor` loads each
      plugin, processes audio, automates a parameter, restores its state
      and opens its editor in our window. Note any editor that draws wrongly,
      won't resize, or hangs.
- [ ] **C. UI timeline.** `ui-timeline --bench 20`, then play with
      `ui-timeline` by hand: scroll, Ctrl+wheel zoom, drag clips, double-click
      to rename, Space to play. Pass: 99% of frames under 16.7 ms and at
      least 55 fps. Also try Windows Narrator on the timeline, and display
      scaling at 150%.
- [ ] **0.6 Speed and pitch, CPU on real hardware.** `stretch-bench` (takes
      a few minutes). Optional: `stretch-bench --input my-playing.wav` with a
      48 kHz recording of real guitar, which makes better listening files.

## Listening

- [ ] **0.6 Speed and pitch.** Compare `original.wav` with each candidate's
      folder in `stretch-listening` (also in the project files under
      `speed-pitch-listening/`). Listen for: pick attacks, warble or
      "phasiness" on sustained chords, and low-string clarity, at 50% and
      75% speed and at −1 semitone. Pick a favourite, or say which are
      unacceptable. Results go in [spikes/README.md](../spikes/README.md).

## Decisions to accept

- [ ] **0003 Implementation language:** confirm Rust once A, B and C pass,
      or fall back to C++ if B or C were a struggle.
- [ ] **0004 Collaboration model:** Loro is preferred for the project
      document (spike 0.5, first half). Accept once the network half has
      been tried on real home, mobile and school networks.
- [ ] **0007 UI toolkit:** choose after the timeline run above. If egui is
      chosen, confirm that its bundled fonts (SIL Open Font Licence 1.1 and
      Ubuntu Font Licence 1.0) can ship, or choose replacement fonts.
- [ ] **0.6 Speed and pitch:** choose a library or our own vocoder after
      listening.
