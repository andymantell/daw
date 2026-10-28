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
latest **ci** run (not the spikes run). It reads WAV files (mono or
stereo); files at another sample rate from the device are converted
first, which takes a moment for long files.

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
- [ ] **1.1 Unplugging.** While playing on the UMC1820, pull its USB
      cable. Pass: the app says the device stopped and keeps running
      (silently); plug it back in and within a few seconds sound returns
      at the same place, without restarting the app. Note what happens
      if not: ASIO drivers differ in how they report this.
- [ ] **1.1 Other sample rates.** Drop a 44.1 kHz WAV file (a CD rip,
      say) next to a 48 kHz one. Pass: "Opening 1 file…" shows briefly,
      then both play in time and at the right pitch. Then change the
      device to 44100 Hz in Settings: both should still play correctly
      after a moment. Converted copies are kept in
      `%LOCALAPPDATA%\daw\converted`; say if that is the wrong place.

### Phase 1 app: recording

Same `daw.exe`. Screenshot of the flow in the cloud (with a test tone
standing in for an interface):
`/mnt/project-files/screenshots/app-recording-takes.png`.

- [ ] **1.2 Input and monitoring.** Settings → Audio device: Input device
      should be the UMC1820 already. Add a track, set its Input to In 1
      (your guitar), press **R** (arm) and **M** (monitor). Pass: you hear
      the guitar through the app, the meter moves, and at 128 samples it
      feels immediate enough to play. Note any crackle.
- [ ] **1.2 Recording.** With a backing track loaded and Count-in on,
      press Record (or the R key), then Space. Play along for a minute
      and stop. Pass: a Take 1 appears and plays back in time with the
      backing. Record again: Take 2 plays, and Take 1 can be chosen back.
- [ ] **1.2 Recording offset.** Run the loopback test from the latency
      spike (review item above) and enter its round trip as the measured
      recording offset. Record a click track through a loopback cable and
      check the take lines up with the original to within a millisecond.
      Also say whether the driver-reported figure the app shows is close.
- [ ] **1.2 Settings kept, and direct monitoring.** Apply the UMC1820
      at 128 samples, close the app and open it again: it should come
      back on the same device and settings. Then tick Monitoring → Direct
      in Settings, turn on the UMC1820's direct monitor, and check you
      hear the guitar once (not doubled) and the M buttons are greyed.
- [ ] **1.2 Crash safety.** While recording, end the app from Task
      Manager. Pass: the take file in `%USERPROFILE%\DAW\Unsaved takes` opens in
      another player and holds everything up to about a second before.

### Phase 1 app: plugins

Same `daw.exe`. Screenshots from the cloud, with the project's test
plugin: `/mnt/project-files/screenshots/app-track-effects.png` and
`app-plugin-list.png`.

- [ ] **1.3 Scanning.** On first start the menu bar shows "Scanning
      plugins: …" for a while. Then open Settings → Plugins. Pass: your
      VST 3 plugins in `C:\Program Files\Common Files\VST3` are listed,
      the app never froze or crashed, and anything under "Could not be
      loaded" has a sensible reason. Start the app again: it should not
      rescan. Say if a plugin you own is missing.
- [ ] **1.3 Amp sim as an insert.** Add a track with your guitar as input,
      arm and monitor it, click **FX** and add an amp sim (any VST 3 you
      have; a free one if not). Pass: you hear the guitar through it,
      **Edit** opens the plugin's own window, turning its knobs changes
      the sound at once, and closing and reopening the window works.
- [ ] **1.3 Chains and bypass.** Add a second effect after the first,
      swap them with ▲▼, toggle **On** off and on, and remove one. Pass:
      no clicks beyond what the change itself causes, and no crash.
- [ ] **1.3 Latency.** With a backing track and a recorded take, put a
      plugin that adds latency (a look-ahead limiter, say; the effects
      window shows "N smp") on the take's track. Pass: it still plays in
      time with the backing.
- [ ] **1.3 Device change.** With effects on a track, switch the device's
      sample rate in Settings → Audio device. Pass: effects keep their
      settings and an open plugin window comes back.

### Phase 1 app: practice

Screenshot (a 220 Hz test tone standing in for a guitar):
`/mnt/project-files/screenshots/app-tuner.png`.

- [ ] **1.4 Tuner.** Practice → Tuner, Input set to your guitar's
      channel. Tune each string, then check against a tuner you trust
      (a clip-on or FL Studio's). Pass: same reading to within a couple
      of cents, steady enough to tune by, and quick to settle after each
      pluck. Try a bass's low E too if one is handy.
- [ ] **1.4 Speed and transpose.** Screenshot:
      `/mnt/project-files/screenshots/app-speed-transpose.png`. Loop a
      tricky bar of a real song, set Speed to 75% then 50%, with the
      metronome on. Pass: it sounds usable for learning (some softening
      of pick attacks is expected), the clicks stay on the slowed beat,
      and the loop joins without a bump. Then try Tuning presets → "Half
      step down → standard" on a song in E♭ tuning and play along in
      standard. This is our own vocoder; compare it with the 0.6 listening
      files when choosing (Decisions below).
- [ ] **1.4 Detected tuning.** On a recording known to be off concert
      pitch (an old vinyl rip, or one at A = 432), Tuning presets →
      "Detect tuning of …". Pass: it reports roughly the right number of
      cents, and "Shift" makes it match your tuned guitar. On a normal
      recording it should say "at concert pitch".
- [ ] **1.4 Tempo trainer.** Loop a short passage at 70%, tick Trainer
      with +5% every 2 loops up to 100%, and play along. Pass: the speed
      box steps up on the loop, smoothly enough to keep playing.
- [ ] **1.4 Practice mute.** Load a backing with a guitar part on its own
      track, press 🎧 on it and play along. Pass: that part goes silent and
      comes back, and Mute still works separately.

### Phase 1 app: export

Screenshot: `/mnt/project-files/screenshots/app-export.png`. In the cloud
the exported mix matched the sum of the stems to within one step of
24-bit, with a plugin on one track.

- [ ] **1.8 Export.** With a backing, a take and an effect on the take's
      track, File → Export with stems ticked. Pass: a new `Mix N` folder
      holds Mix.wav and one file per track, they open in another player,
      the mix sounds like playback (effects included) and the stems line
      up when dropped into FL Studio.
- [ ] **1.8 Export folder.** Export from a saved project, then from a
      new unsaved one, then use Choose folder…. Pass: the saved project's
      mixes land in its own `Exports` folder, the unsaved one's in
      `%USERPROFILE%\DAW\Exports`, and Choose folder… opens the Windows
      folder window and exports there (screenshot:
      `app-export-into-project.png`).

### Phase 1 app: editing

Screenshots: `/mnt/project-files/screenshots/app-timeline.png` (split,
trimmed and moved clips, three renamed markers, the Verse section
looped) and `app-timeline-marker-menu.png`. In the cloud the timeline
was driven with mouse and keys; how it feels is for you.

- [ ] **1.5 Clip editing.** Drop in a backing and a guitar file. Click
      the ruler to move the playhead, select a clip and press S to split,
      drag a clip to move it, drag its edges to trim, Delete to remove.
      Pass: playback follows each edit, edges and moves land on beats
      (bars with Snap → Bar, anywhere with Alt held), and nothing clicks
      or drops out at the cut points.
- [ ] **1.5 Undo.** After a few edits, Ctrl+Z back to the start and
      Ctrl+Shift+Z (or Ctrl+Y) forward again. Pass: each step is one
      edit, and playback matches what you see.
- [ ] **1.5 Sections.** Press M at the start of each part of a song,
      double-click a marker to name it, right-click → Loop this section.
      Pass: the loop covers that section only. Say if markers need to sit
      between bar lines (they snap to bars for now).
- [ ] **1.5 Takes on the timeline.** Record two takes over the same bars.
      Pass: the newer one plays and the older one shows as an outline;
      the Take menu brings the older one back; Ctrl+Z after recording
      removes the take (its file stays in Unsaved takes).
- [ ] **1.5 Feel.** Zoom with Ctrl + wheel, scroll with the wheel, and
      drag clips around a long song. Say if it is smooth enough and what
      feels wrong.

### Phase 1 app: performance

Screenshot: `/mnt/project-files/screenshots/app-performance-check.png`
(the cloud machine, a debug build with no audio device, so its numbers
mean nothing).

- [ ] **1.9 Performance bar.** On the UMC1820 with the buffer at 128
      (Settings → Audio device), open Settings → Performance check, add
      the 16 test tracks, add a real EQ or compressor to 8 tracks, loop
      and play for five minutes while zooming the timeline and opening
      plugin windows. Pass: Dropouts stays at none, nothing crackles, and
      the DSP load is well under 100%. Note the load and peak figures
      either way, and try 64 if 128 passes easily.

### Phase 1 app: projects

Screenshots: `/mnt/project-files/screenshots/app-save-project.png`,
`app-open-project.png`, `app-project-reopened.png` (tracks, split clips,
the Section A marker, a recorded take and a plugin's gain all came
back) and `app-lesson-notes.png`. Projects live in
`%USERPROFILE%\DAW\Projects`, one folder each.

- [ ] **1.6 Save and open.** Drop in a backing and a guitar file, record
      a take, split a clip, add a marker and a plugin with a changed
      setting, then Ctrl+S and give it a name. File → New project, then
      File → Open project… and click it. Pass: everything comes back as
      it was, including the plugin's setting, and the window title shows
      the project's name.
- [ ] **1.6 System file windows.** File → Open project… → Browse…, and
      in the Save window, Somewhere else…. Pass: the normal Windows
      folder and Save windows open (they could not be tried in the
      cloud), a project folder picked anywhere opens, and a project
      saved somewhere else becomes a folder of that name there.
- [ ] **1.6 Audio in the folder.** Look in the project's folder. Pass:
      `project.daw`, `project.daw.bak` after a second save, and an
      `Audio` folder holding a copy of each imported file and the
      recorded takes (moved out of `DAW\Unsaved takes`, which is left
      empty). Say if a take was copied rather than moved: on Windows a
      file still open for playback may not move.
- [ ] **1.6 Autosave and quitting.** Make an edit to a saved project, wait
      30 seconds, and check `project.daw`'s modified time. Then make an
      edit and close the window. Pass: the file updates by itself, and
      closing an unsaved new project asks to save, discard or cancel.
- [ ] **1.6 Undo through the document.** Repeat the 1.5 Undo check on a
      saved project, then save, reopen and undo. Pass: undo works as
      before; after reopening there is nothing to undo (history is per
      run of the app, as decision 0004 says).
- [ ] **1.7 Lesson notes.** Practice → Lesson notes…, type a few lines,
      save, reopen. Pass: the notes come back. They are plain text for
      now; say what you would want first: headings, tab blocks pinned
      to sections, or something else.

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
- [x] **0004 Collaboration model:** Loro accepted on 2026-10-28. The
      network half is still to be tried on real home, mobile and school
      networks (see "On other networks").
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
