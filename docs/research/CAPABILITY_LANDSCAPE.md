# Capability Landscape

A survey of capabilities that professional DAWs and hardware loop stations
commonly offer, written **in generic terms** as allowed by
[CLEAN_ROOM.md](../CLEAN_ROOM.md). It does not describe or replicate any
specific product. Each capability is mapped to the roadmap phase where we plan
to deliver *our own* version of it.

How to read it:

- **Phase**: where it lands in [ROADMAP.md](../ROADMAP.md) (`1` = vertical
  slice, `—` = not planned yet).
- **Web**: how feasible it is in a browser today. ✅ straightforward ·
  ⚠️ possible with caveats · 🖥️ needs the optional desktop wrapper for full
  quality.

Last researched: October 2026. Browser-support statements must be checked
again before each phase starts.

---

## 1. Recording

| Capability | Phase | Web | Notes |
|---|---|---|---|
| Select input device and channel(s) | 1 | ✅ | `getUserMedia` with device ID; browser DSP (echo cancel, noise suppress, AGC) must be disabled for music. |
| Arm track, record, count-in, metronome | 1 | ✅ | |
| Record-latency compensation (place takes correctly in time) | 1 | ⚠️ | Use loopback calibration; `AudioContext.outputLatency` helps but differs between browsers. |
| Punch in/out (manual and range-based) | 2 | ✅ | |
| Pre-roll / post-roll | 2 | ✅ | |
| Loop recording into stacked takes | 2 | ✅ | |
| Take lanes and comping (quick swipe comp, automatic crossfades) | 2 | ✅ | |
| Retrospective capture (keep input you played while not recording) | 3 | ✅ | Ring buffer of recent input; very useful for students. |
| Multi-input simultaneous recording | 2 | ⚠️ | Multichannel `getUserMedia` support varies by browser/OS. |
| Low-latency software monitoring through effects | 8 | 🖥️ | Browsers on Windows use shared-mode WASAPI; direct/hardware monitoring is the web answer. |

## 2. Audio editing

| Capability | Phase | Web | Notes |
|---|---|---|---|
| Move, trim, split, duplicate, nudge clips; snap to grid | 1–2 | ✅ | Phase 1 has the minimal set. |
| Fades and crossfades with curve shapes | 2 | ✅ | |
| Clip gain and gain envelopes | 2 | ✅ | |
| Non-destructive editing with unlimited, collaborative undo | 1–2 | ✅ | Per-user undo is a CRDT design concern. |
| Normalise, reverse, (destructive) offline processing | 2 | ✅ | Produce new assets; never mutate the source. |
| Tempo-following audio (stretch to project tempo) | 6 | ⚠️ | Several algorithm families; quality and CPU cost differ. |
| Manual stretch markers on transients | 6 | ⚠️ | |
| Transient detection, slicing to clips/sampler | 6 | ✅ | |
| Audio quantise (align transients to grid) | 6 | ⚠️ | |
| Tempo detection / build tempo map from free recording | 6 | ⚠️ | |
| Monophonic pitch editing (note-level pitch/vibrato/formant) | 7 | ⚠️ | Hard DSP; heavy research phase. |
| Spectral view / spectral repair | 9 | ⚠️ | |
| ML stem separation (drums/bass/vocals/other…) | 7 | ⚠️ | WebGPU/WASM inference; model + training-data licences must be clean. |
| Audio-to-MIDI (mono, then poly) | 7 | ⚠️ | |
| Chord detection from audio/MIDI to chord track | 7 | ⚠️ | |

## 3. Practice & education (our emphasis)

| Capability | Phase | Web | Notes |
|---|---|---|---|
| Loop a region; A/B repeat | 1 | ✅ | |
| Slow down/speed up without pitch change | 1 | ⚠️ | Needs a stretch implementation in a worklet (licence check). |
| Transpose playback without tempo change | 1 | ⚠️ | Same engine as above. |
| Chromatic tuner | 1 | ✅ | Pitch detection on input. |
| Metronome with accents, subdivisions, gradual tempo trainer | 1 | ✅ | Tempo trainer: auto speed up each N loops. |
| Comments pinned to time/track | 1 | ✅ | |
| Practice log (time practised, tempo reached) | 3 | ✅ | |
| Tab view / simple notation of notes | 3 | ✅ | Start with text tab in Phase 1. |
| Isolate/remove a part from a backing track | 7 | ⚠️ | Uses stem separation. |
| Live lesson call (video + music-quality audio) | 4 | ⚠️ | WebRTC; disable voice DSP, raise Opus bitrate. |

## 4. Mixing

| Capability | Phase | Web | Notes |
|---|---|---|---|
| Volume, pan, mute, solo, meters | 1 | ✅ | |
| Insert effect chains with bypass and reorder | 2 | ✅ | |
| Sends, effect returns, group buses | 2 | ✅ | |
| Control-group faders (VCA-style) | 2 | ✅ | |
| Sidechain routing with latency compensation | 2 | ✅ | |
| Automatic processing-delay compensation | 2 | ✅ | Engine-wide, including sends and sidechains. |
| Automation: draw, record (read/touch/latch/write), clip-based | 2 | ✅ | |
| Mixer view embedded in the main editing window | 2 | ✅ | |
| Channel strip presets, mixer snapshots | 3 | ✅ | |
| Loudness/level metering (LUFS, true peak), spectrum, phase correlation | 2–3 | ✅ | |
| Freeze / render in place | 3 | ✅ | |
| Surround / immersive object mixing | 9 | ⚠️ | Binaural monitoring possible in browser; formats may involve licensing. |

## 5. Built-in effects

| Capability | Phase | Web | Notes |
|---|---|---|---|
| EQ (parametric, filters), compressor, gate, limiter | 2 | ✅ | |
| Reverb (algorithmic + convolution), delay | 2 | ✅ | Convolution node built into Web Audio. |
| Guitar amp/cab simulation (waveshaping + cabinet IRs) | 2 | ✅ | Our own IRs only. |
| Modulation (chorus, flanger, phaser, tremolo, auto-pan) | 3 | ✅ | |
| Distortion/saturation with multiple stages, multiband | 3 | ✅ | |
| Transient shaper, de-esser, multiband dynamics | 3 | ✅ | |
| Pitch shifter, harmoniser, vocoder | 5–7 | ⚠️ | |
| Performance effects (beat repeat, roll, stutter, freeze, filter sweeps, lo-fi, vinyl-style) | 5 | ✅ | Used heavily in Live Looper mode. |
| Parameter modulators (LFO, step, random, sample & hold, envelope follower) | 7 | ✅ | |

## 6. MIDI & instruments

| Capability | Phase | Web | Notes |
|---|---|---|---|
| MIDI input from controllers | 3 | ⚠️ | Web MIDI works in Chromium and Firefox; **not natively in Safari** (as of 2026 research). |
| Note editor with velocity/CC lanes, quantise, humanise | 3 | ✅ | |
| Step sequencer / drum pattern editor with swing, probability, Euclidean fills | 3 | ✅ | |
| Melodic pattern sequencer with scales | 7 | ✅ | |
| Scale awareness (key/scale guides editing) | 3 | ✅ | |
| Chord track driving tools/generators | 7 | ✅ | |
| Note transformations (strum, arpeggiate, ornament, humanise) | 7 | ✅ | Strum is very guitar-relevant. |
| Constrained generators (rhythm, melody, chords) | 9 | ✅ | Rule-based first; any ML must have clean data. |
| Sampler (multi-zone, round robin), drum sampler | 3 | ✅ | |
| Synthesiser(s) | 3 | ✅ | |
| MPE and MIDI 2.0 | 7 | ⚠️ | Depends on browser API evolution. |
| Microtonal tuning (scale files) | 9 | ✅ | |
| Articulation/expression maps | 9 | ✅ | |
| Score/notation editor | 9 | ⚠️ | Large project in its own right; tab first. |
| Virtual/AI accompanist players | — | ⚠️ | Research only; strong clean-data requirement. |

## 7. Non-linear / performance

| Capability | Phase | Web | Notes |
|---|---|---|---|
| Clip Grid with quantised launch, scenes, follow actions | 6 | ✅ | |
| Capture grid performance to timeline | 6 | ✅ | |
| Pattern-based song building (patterns placed on timeline) | 3 | ✅ | |
| Controller mapping (MIDI learn) | 3 | ⚠️ | Web MIDI caveat. |

## 8. Loop-station capabilities (Live Looper mode)

Generic capabilities found on multi-track loop stations, as a target list for
our own mode. Our implementation must show everything on a visible timeline
and in a visible mixer, not only behind physical buttons.

| Capability | Phase | Notes |
|---|---|---|
| Several (e.g. 5+) stereo loop tracks, each with level/pan | 5 | We are not limited to a fixed number. |
| Record → overdub → play cycle on one control; configurable order (rec→dub or rec→play) | 5 | |
| Overdub modes: layer, replace, punch-in replace | 5 | |
| Per-track undo/redo of last layer; "bookmark" a layer to return to | 5 | |
| Loop sync: tracks aligned to a master loop length vs. free length | 5 | |
| Tempo sync on/off; pitch follows tempo or not | 5 | |
| One-shot (play once) vs loop | 5 | |
| Reverse playback | 5 | |
| Quantised start/stop (to beat/bar/loop end) | 5 | |
| Start/stop modes: immediate, fade, at loop end | 5 | |
| Play mode: multi (all play) vs single (one at a time, e.g. verse/chorus) | 5 | |
| Auto-record when input level passes a threshold | 5 | |
| Input effects (before recording) and track effects (on playback), several at once, in recallable banks | 5 | |
| Performance effects: filters, beat repeat, roll, freeze, pitch bend, harmony, vocoder, lo-fi, radio, robot | 5 | Implemented as our own processors. |
| Built-in rhythm/drum patterns with kits | 5 | Our own patterns and samples only. |
| Bounce: record one track while playing others into it | 5 | |
| Saved phrase sets (memory slots) | 5 | |
| Footswitch / expression pedal / MIDI control, press/hold/double-press actions | 5 | Keyboard and Web MIDI first; HID later. |
| Long recording time per track | 5 | Bounded by browser storage quotas; stream to disk (OPFS). |

## 9. Project, files & interchange

| Capability | Phase | Web | Notes |
|---|---|---|---|
| Autosave, local-first editing, offline work | 1 | ✅ | IndexedDB/OPFS + CRDT. |
| Project snapshots/versions | 1–2 | ✅ | Manual snapshots in Phase 1; diff summaries in Phase 2. |
| Import audio (WAV, AIFF, FLAC, MP3, Ogg/Opus) | 1–2 | ✅ | Decode via `decodeAudioData` or WASM decoders. |
| Export mixdown (WAV first; then FLAC, MP3, Opus) and stems | 1–3 | ✅ | |
| Standard MIDI file import/export | 3 | ✅ | |
| Open DAW-interchange format import/export (DAWproject, MIT-licensed spec) | 4 | ✅ | |
| Templates | 2 | ✅ | |
| Track/arrangement versions (alternate takes of a whole track) | 4 | ✅ | |
| Video track for scoring/sync | 9 | ⚠️ | |
| Content browser with tags, search, preview | 3 | ✅ | |
| Search by sound similarity | — | ⚠️ | Research. |

## 10. Extensibility & platform

| Capability | Phase | Web | Notes |
|---|---|---|---|
| Internal processor API (our devices built on it) | 2 | ✅ | |
| Host web plugins (Web Audio Modules 2.0, open standard) | 7 | ✅ | |
| Web CLAP (proposal) | — | ⚠️ | Watch; adoption unclear as of research. |
| Native plugin hosting (VST 3 — MIT SDK; CLAP) | 8 | 🖥️ | Desktop wrapper only. |
| ASIO / CoreAudio / JACK low-latency I/O | 8 | 🖥️ | Desktop wrapper only; ASIO SDK GPLv3 or proprietary. |
| User scripting (editor actions, MIDI processing) | 7 | ✅ | Sandboxed. |
| Controller scripting / surfaces | 7 | ⚠️ | |
| Customisable key commands with search | 2 | ✅ | |
| Built-in help assistant | — | ✅ | Later; must use our own docs. |

## 11. Collaboration (our differentiator)

| Capability | Phase | Notes |
|---|---|---|
| Share link with roles | 1 | |
| Real-time co-editing of project state | 1 | CRDT. |
| Presence (who's here, their playhead) | 1 | |
| Pinned comments and threads | 1 | |
| Follow mode (my view follows the teacher's playhead) | 1 | |
| Lesson history / snapshots per lesson | 3 | |
| Review mode: suggest edits, accept/reject | 4 | |
| Live lesson call | 4 | |
| Real-time remote jamming | — | Fundamentally limited by network latency; possible interval-delay mode later. |
