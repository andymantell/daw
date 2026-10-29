# Research: scoring to picture

Checked 2026-10-29, for a feature request: embed a video file in a project
so music can be written to it. Online video links are covered separately in
[VIDEO_STEMS_REPITCH.md](VIDEO_STEMS_REPITCH.md); this page is about files.

## Findings

### Decoding without shipping codecs

- **Windows** ships video decoders in its media framework: H.264 (Baseline,
  Main and High profiles to level 5.1), H.265/HEVC (Main, Main 10), MPEG-4
  Part 2, DV and the older Windows Media codecs. The framework's "source
  reader" hands an app decoded frames one at a time. The page we read does
  not list VP9 or AV1 and does not say which decoders need an optional
  package. *(Source: Microsoft Learn, "Codec objects" and "Processing media
  data with the Source Reader", read 2026-10-29.)*
- **macOS** has an asset reader that reads media data from QuickTime and
  MPEG-4 files, with a settable time range and random-access reading of
  sample buffers. *(Source: Apple developer documentation, `AVAssetReader`,
  read 2026-10-29.)* The list of supported codecs is on other pages we have
  not read yet; H.264, HEVC and ProRes are expected but unverified here.
- **Linux** has a widely used LGPL media framework whose codec plugins are
  installed by the distribution. Which decoders are present varies by
  distribution and user choice. *(Not verified against official pages
  yet.)*
- **Consequence:** decode through the operating system on Windows and
  macOS, and through the system media framework on Linux. We ship no codec
  code, so no codec patent licences and no GPL-compatibility questions
  about decoder libraries. If a file uses a codec the system lacks, say so
  and offer to convert it with a tool the user has.

### Timecode and frame rates

- SMPTE timecode labels frames as hours:minutes:seconds:frames at 23.976
  (24/1.001), 24, 25, 29.97 (30/1.001) and 30 frames per second.
  Drop-frame applies to 29.97 and 30: frame numbers 0 and 1 are skipped at
  the start of every minute except every tenth minute, so the count stays
  in step with the clock. No picture frames are removed. Drop-frame is
  usually written with a semicolon before the frame count. *(Source:
  Wikipedia, "SMPTE timecode", read 2026-10-29. To be confirmed against the
  standard itself before implementation.)*
- The project's timing model (decision 0005) is ticks plus sample frames.
  Timecode is a display and snapping layer on top of sample time, not a
  third clock. The frame shown is the one whose span contains the playhead
  sample.

### Scoring workflow needs

- A video window that is frame-accurate when playing, scrubbing and
  stepping by frame.
- Hit points on frames, and a tempo-fit tool that picks a tempo or tempo
  change so hit points fall on beats, reporting the error in frames.
- The video's start timecode and an offset, so project time and film time
  agree with the video editor's.
- Export that wraps the mix into a copy of the video without re-encoding
  the picture. This needs a container writer, not an encoder.

## Not researched yet

- The exact codec list for the macOS asset reader, and for the Linux
  framework's common plugin sets.
- Whether the system frameworks give presentation timestamps precise
  enough for frame-accurate seeking in long files, or whether we must
  build a frame index on import.
- Container writing (MP4/MOV) without re-encoding, on each OS.
- Reading timecode tracks embedded in files, and reel names.
