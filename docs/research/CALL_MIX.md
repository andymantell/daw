# Research: sending the app's sound into a video call

Checked 2026-11-07, for a feature request: the app's output should
optionally also go to a virtual audio output, with the owner's headset
microphone mixed in, so a video-call app can use that as its microphone
and hear both the music and the owner.

## What the request needs

Two things, one of which the app can do alone:

1. **A call mix inside the app:** the main mix (or a headphone mix) plus a
   chosen input (the headset microphone), each with its own level and
   mute, sent to a chosen output as an extra pair. The engine already has
   headphone mixes and talkback (monitor section, roadmap 2.3), extra
   output pairs, and an input stream that can be a different device from
   the output stream, so this is the same shape again: one more output
   stream and one more input stream, with a ring buffer each.
2. **A virtual audio device** that the call app can pick as its
   microphone. Whether the app can provide one depends on the platform.

## Virtual audio devices by platform

- **Windows:** a device that other, unmodified applications can see
  must come from a kernel-mode driver; the audio engine only enumerates
  kernel devices, and user-mode sources only work for applications
  written to look for them. *(Source: answers by a Windows driver
  developer on a driver-development forum, November 2024 and February
  2025, read 2026-11-07.)* Shipping our own kernel driver would need a
  signed driver and Microsoft's attestation process, which is a project
  in itself. **Consequence:** on Windows the app relies on a virtual
  audio cable driver that the owner installs (several free and paid ones
  exist); the app lists it as an ordinary output device, and the guided
  audio setup explains the step and recognises when one is present.
- **macOS:** Core Audio supports **Audio Server plug-ins**, user-space
  drivers that publish a virtual device; the maker's sample installs a
  `.driver` bundle into `/Library/Audio/Plug-Ins/HAL` and needs a
  restart. *(Source: the platform's developer documentation sample
  "Creating an Audio Server Driver Plug-in", read 2026-11-07.)* So the
  app could ship its own virtual device later, at the cost of an
  installer step with administrator rights. First cut: the same
  third-party route as Windows. The test rig is Windows only, so this
  waits anyway.
- **Linux:** the sound server (PipeWire or PulseAudio) can create a
  virtual sink and loopback from user space with no driver, so the app
  can make its own "call" device when the feature is turned on. *(Known
  from the sound servers' own tooling; not verified in this pass.)*

## Two devices at once

The call output is usually a different device from the music interface
(a virtual cable, or the headset), and the headset microphone is a
third. Devices run on their own clocks, so the extra streams drift
against the engine's. The input stream already crosses devices through
a ring buffer; the call output needs the same, with drift handled by
dropping or repeating a sample now and then (inaudible on a call) or a
small resampler. Latency on the call path does not matter the way
monitoring latency does; a buffer of a few tens of milliseconds is fine.

## The call app's side

Video-call apps process the microphone for speech: noise suppression,
echo cancellation, automatic gain. For music the owner turns on the
call app's "original sound" or "high-fidelity music" setting, which
drops that processing, and optionally stereo. *(Source: a music
school's help page on configuring a widely used call app for music
performance, read 2026-11-07.)* The app can show this as a reminder when
the call mix is turned on, without naming any product.

## What the mode would look like

A **Call** section in the monitor pane: on/off, output device (pair),
microphone device and channel, mic level and mute, music level and
mute, which mix (main or a headphone mix), a meter, and a one-line
reminder about the call app's music setting. The setting is per
computer, like the rest of the device profile. Commands for on/off,
mic mute and levels, so an assistant can run it.

## Later

- Hearing the far end inside the app (a "call in" input) and recording
  the lesson, both sides on their own tracks.
- Our own virtual device on macOS; on Windows only if a signed driver
  ever becomes worth it.
