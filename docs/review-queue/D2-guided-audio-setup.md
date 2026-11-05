# D2 Guided audio setup

Settings → **Guided audio setup…** walks through the audio settings a
step at a time, and opens by itself the first time the app starts with
no device settings saved. The Audio device window has a **Guided
setup…** button too. The steps:

1. **Your audio interface.** Lists every device that can play, best
   first: one on the interface maker's own (ASIO) driver, then one that
   can also record, then the rest. Each says why. **Use this** opens it
   at 48 kHz. On Windows with only the shared system drivers, it
   explains that the maker's ASIO driver has less delay. If the device
   won't start, it says why and what usually causes it (another program
   using it, a setting the driver doesn't offer).
2. **How you listen while playing.** Through the app (hear the amp and
   effects, needs a small buffer) or through the interface's direct
   monitoring (no delay, no effects; the app stops playing the input
   back so you don't hear it twice).
3. **Buffer size.** Suggests 128 samples when listening through the app,
   256 with direct monitoring, within what the driver allows. **Try it**
   listens for a few seconds; if the sound broke up it says how often
   and picks the next size up. It also says how late you'd hear
   yourself through the app, and warns above about 10 ms.
4. **Input level.** Pick the input and play loud; a meter and a plain
   judgement say whether nothing is coming in (check the cable, gain,
   48 V for a condenser microphone, the instrument switch for a guitar),
   too quiet, good, nearly too loud, or clipping.
5. **Recording delay.** Explains the loopback cable and opens the
   existing Measure latency window; shows the result for these settings.
6. **All set.** A summary of the settings, and anything still worth
   fixing.

Every step is a command in the shared table (`audio-setup`,
`audio-setup-next`, `-skip`, `-back`, `-cancel`), plus `measure-latency`
to open the measurement, so an assistant can walk it; a test does so
through the agent driver.

Screenshots, from the cloud with a made-up interface (no real device),
in `/mnt/project-files/screenshots/`: `app-audio-setup-device.png`,
`app-audio-setup-monitoring.png`, `app-audio-setup-buffer.png` (with a
dropout warning shown), `app-audio-setup-input-quiet.png`,
`app-audio-setup-latency.png` and `app-audio-setup-done.png`.

## On the PC, with the interface

- [ ] **First run.** Rename or move the settings file
      (`%APPDATA%\<app>\settings.txt`) and start the app. Pass: the guide
      opens by itself, once; after finishing or closing it, it doesn't
      open again on the next start.
- [ ] **The interface is offered first.** Pass: the interface on its
      ASIO driver is at the top. Say what it is called there, and
      whether the shared-driver entries for it show as able to record.
- [ ] **Buffer check.** Pick 128 samples and press Try it. Pass: it moves
      on to Input level after about 3 seconds. Then try 32 or 64 with a
      busy project open: if the sound breaks up, the guide should say so
      and pick a bigger size. The check plays nothing itself, so a quiet
      computer may pass sizes that break up once a project plays: say if
      that happens.
- [ ] **Input level.** Plug the guitar into input 1 and play. Pass: the
      advice follows the gain knob (quiet → good → nearly too loud →
      clipping), and matches the interface's own clip light.
- [ ] **Monitoring delay.** With "through the app" at 128 samples, the
      guide shows how late you'd hear yourself. Say whether that number
      feels right, and whether direct monitoring sounds doubled (it
      shouldn't).
- [ ] **Latency.** Run the measurement from step 5 with a loopback
      cable. Pass: step 5 and the summary show the measured frames.

## Decisions

- [x] **Thresholds.** Quiet below −24 dBFS peak, nearly too loud above
      −3 dBFS, clipping at −0.1 dBFS, silent below −60 dBFS, and "feels
      late" above 10 ms. These are rules of thumb chosen here, not taken
      from a standard: accept or give your own.
- [x] **Default way of listening.** The guide starts on "through the
      app" (so the amp is heard while playing). Accept, or start on
      direct monitoring.
