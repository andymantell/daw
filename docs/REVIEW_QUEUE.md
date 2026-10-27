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
      and opens its editor in our window. Instruments are sent notes. Note any
      editor that draws wrongly, won't resize, or hangs, and any instrument
      the report calls SILENT.
- [ ] **B. Plugins in separate processes.** Pick two effect plugins from
      the scan and run
      `plugin-host oop "<first>.vst3" "<second>.vst3" --block 128`.
      Pass: no blocks over budget or late, and "audio flowing again after
      restart" after the deliberate crash.
- [ ] **C. UI timeline (egui).** `ui-timeline --bench 20`, then play with
      `ui-timeline` by hand: scroll, Ctrl+wheel zoom, drag clips,
      double-click to rename, Space to play. Pass: 99% of frames under
      16.7 ms and at least 55 fps. Also try Windows Narrator on the
      timeline, and display scaling at 150%.
- [ ] **C. Plugin editor inside the window.** `ui-timeline --plugin
      "C:\Program Files\Common Files\VST3\<name>.vst3"` with two or three
      of your plugins. Check that the editor appears in the bottom panel,
      its knobs work with the mouse and keyboard, it stays in place when
      you resize the window, and the timeline stays smooth while it is
      open. Note any plugin that looks wrong or crashes.
- [ ] **0.4 Latency.** Connect output 1 to input 1 of the UMC1820 with a
      short jack cable, start with input 1's gain low (raise it if the report
      says no pulses were heard), and turn the monitors down (the test
      pulses are short chirps). Run
      `latency --device UMC --buffers 64,128,256 --repeats 5`.
      Pass: each buffer size says STABLE or WITHIN A BLOCK.
- [ ] **0.6 Speed and pitch, CPU on real hardware.** `stretch-bench` (takes
      a few minutes). Optional: `stretch-bench --input my-playing.wav` with a
      48 kHz recording of real guitar, which makes better listening files.

### Phase 1 app (first window)

The app itself, `daw.exe`, is in the **daw-windows-x64** artifact of the
latest **ci** run (not the spikes run). It only reads WAV files at the
same sample rate as the audio device for now.

- [ ] **1.1 Device and playback.** Open `daw.exe`, then Settings → Audio
      device. Check that ASIO and the UMC1820 are listed, choose them at
      48000 Hz and 128 samples, and Apply. Drop two or three WAV files
      (48 kHz) on the window and play them for ten minutes. Pass: no
      crackles, the meters move, and no yellow "disk too slow" warning.
- [ ] **1.1 Mixer and transport.** Move volume and pan while playing
      (smooth, no clicks), try mute and solo, loop bars 1 to 5, and Space
      to start and stop.
- [ ] **1.1 Metronome.** Turn the metronome on and check it feels on the
      beat against a backing track you know the tempo of. Try Eighths,
      Count-in, and Metronome output set to outputs 3/4 so the click is in
      the headphones only.

## On other networks

Spike 0.5, live sessions without a server
([decision 0004](decisions/0004-collaboration-model.md)). `collab-net.exe`
is in the same download.

- [ ] **Network probe, three places.** Run `collab-net probe` at home, on a
      laptop tethered to a phone hotspot, and if possible on a school or
      office network. Each run saves `net-probe-report.txt`; keep all three.
- [ ] **A real two-machine session.** Teacher machine at home runs
      `collab-net host`; a second machine (a student, or a laptop on a
      phone hotspot) runs `collab-net join <invite>`. Paste the reply code
      back into the teacher's window. Pass: both sides say "both edits
      arrived". Try once on the same home network, and once with the
      second machine on the hotspot.
- [ ] **Windows firewall.** The first run may show a firewall prompt for
      `collab-net`. Note what it says and whether allowing it was needed.

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
- [ ] **Public STUN servers:** live sessions ask free public STUN servers,
      run by others, for each machine's outside address. They see addresses
      only, never project data. Confirm this fits "no server we operate", or
      say if it should be optional (manual addresses only).
- [ ] **0007 UI toolkit, egui fonts:** egui was chosen on 2026-10-27.
      Confirm that its bundled fonts (SIL Open Font Licence 1.1 and
      Ubuntu Font Licence 1.0) can ship, or choose replacement fonts.
- [ ] **0.6 Speed and pitch:** choose a library or our own vocoder after
      listening.
- [ ] **0.7 Windows signing:** ship unsigned test builds now, apply to a
      free open-source signing service after the first public release, and
      fall back to an OV certificate if refused. See
      [PLATFORM_DISTRIBUTION.md](research/PLATFORM_DISTRIBUTION.md).
- [ ] **0.7 macOS:** join the Apple Developer Program (99 USD a year, no fee
      waiver for individuals) before any macOS release, or say no macOS
      release for now. No macOS test machine exists yet either.
- [ ] **0.7 Minimum OS versions:** Windows 10 22H2 and 11; macOS 13+
      universal (Intel dropped when its security updates end); current
      Ubuntu LTS for Linux tests.
- [ ] **0.7 Intel-only plugins on Apple Silicon:** support them through a
      Rosetta helper only while macOS allows it, with a warning that they
      will stop working.
- [ ] **0.7 Updates:** opt-in daily check of the release page, notify only,
      with signature checking; no self-install in version 1.
- [ ] **0.7 Linux packaging:** AppImage plus `.tar.gz` first; Flatpak after
      version 1.
