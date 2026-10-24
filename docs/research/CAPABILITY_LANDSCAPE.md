# Capability Landscape

A survey of capabilities that professional DAWs and hardware loop stations
commonly offer, written **in generic terms** as allowed by
[CLEAN_ROOM.md](../CLEAN_ROOM.md). It does not describe or replicate any
specific product. Each capability is mapped to the roadmap phase where we
plan to deliver *our own* version of it.

- **Phase** refers to [ROADMAP.md](../ROADMAP.md): `1` is the Lesson Room
  vertical slice, and `—` means not planned yet.
- **Risk** is technical difficulty and uncertainty: L (low), M (medium),
  H (high).

Last researched: October 2026. See [Research gaps](#research-gaps) at the
end for what could not be verified.

---

## 1. Recording & monitoring

| Capability | Phase | Risk | Notes |
|---|---|---|---|
| Choose driver, device, sample rate, buffer size | 1 | M | ASIO/WASAPI/CoreAudio/ALSA/JACK. |
| Input selection per track, arm, record, count-in, metronome | 1 | L | |
| Recording-offset compensation | 1 | M | Measured by loopback; driver-reported values can be wrong. |
| Low-latency software monitoring through effects/plugins | 1 | M | Core reason for going native. |
| Punch in/out, pre-roll and post-roll | 2 | L | |
| Loop recording into stacked take lanes | 2 | M | |
| Comping: swipe-select best parts, auto crossfades, audition, flatten | 2 | M | |
| Multi-input simultaneous recording | 2 | L | |
| Retrospective capture (keep input played while not recording) | 2 (audio), 3 (MIDI) | L | Valuable for students. |
| Monitor section: control-room output, cue/headphone mixes, talkback, dim/mono, speaker switching | 2 | M | Useful for in-person lessons. |

## 2. Audio editing

| Capability | Phase | Risk | Notes |
|---|---|---|---|
| Move, trim, split, delete, snap | 1 | L | |
| Nudge, slip, ripple, range tools, zoom | 2 | L | |
| Fades and crossfades with curves; clip gain and envelopes | 2 | L | |
| Group/linked editing across tracks (e.g. multi-mic drums) | 2 | M | |
| Non-destructive editing with collaborative undo | 1 | M | Per-user undo over the CRDT. |
| Offline processing (normalise, reverse, gain) creating new assets | 2 | L | |
| Audio follows project tempo (stretch) | 6 | H | Quality vs CPU; licence-sensitive. |
| Stretch markers; transient detection; slicing | 6 | M | |
| Audio quantise to grid | 6 | M | |
| Tempo detection / tempo map from free performance | 6 | M | |
| Monophonic pitch editing (pitch, drift, vibrato, formant) | 7 | H | |
| Plugin-based direct audio editing via ARA 2 | 7 | M | Open SDK (Apache 2.0). |
| ML stem separation | 7 | H | Model weights *and* training data must be licensed for our use. |
| Audio-to-MIDI (mono, then poly); chord detection | 7 | H | |
| Spectral view and repair | 8 | H | |

## 3. Practice & education (our emphasis)

| Capability | Phase | Risk | Notes |
|---|---|---|---|
| Loop a region / section | 1 | L | |
| Speed change without pitch change; transpose without tempo change | 1 | M | Spike 0.6. |
| Chromatic tuner | 1 | L | |
| Metronome with accents, subdivisions; tempo trainer | 1 | L | |
| Comments pinned to time/track | 1 | L | |
| Text tab blocks | 1 | L | |
| Practice log; assignments with progress | 3 | L | |
| Tab view and entry for notes | 3 | M | |
| Isolate/remove a part from a backing track | 7 | H | Uses stem separation. |
| Live lesson call (audio/video, music-quality audio) | 4 | M | Peer-to-peer. |

## 4. Mixing & routing

| Capability | Phase | Risk | Notes |
|---|---|---|---|
| Volume, pan, mute, solo, meters | 1 | L | |
| Insert chains (bypass, reorder) | 1 (plugins), 2 (full) | L | |
| Sends, returns, group buses, folder tracks | 2 | L | |
| Control-group faders (VCA-style) | 2 | L | |
| Sidechain routing | 2 | M | |
| Engine-wide delay compensation (incl. sends, sidechains, plugins) | 1 (per track), 2 (global) | M | |
| Automation (draw; read/touch/latch/write), clip automation | 2 | M | |
| Mixer embedded in main window and as a full window | 2 | L | |
| Metering: LUFS, true peak, spectrum, phase correlation | 2 | L | |
| Channel strip presets, mixer snapshots | 3 | L | |
| Freeze / render in place | 3 | M | Frozen audio for missing plugins is Phase 1. |
| Surround / immersive mixing | 8 | H | Branded formats may need licences. |

## 5. Built-in effects

| Capability | Phase | Risk | Notes |
|---|---|---|---|
| EQ, compressor, gate, limiter, reverb (algorithmic + convolution), delay | 2 | M | |
| Guitar amp & cabinet simulation | 2 | M | Our own models and impulse responses. |
| Modulation (chorus, flanger, phaser, tremolo, auto-pan) | 3 | L | |
| Multi-stage/multiband saturation, transient shaper, de-esser, multiband dynamics | 3 | M | |
| Pitch shifter, harmoniser, vocoder | 5–7 | M | |
| Performance effects (beat repeat, roll, stutter, freeze, filter sweeps, lo-fi, radio, robot, tape stop) | 5 | M | |
| Modulators (LFO, step, random, sample & hold, envelope follower) | 7 | M | |

## 6. MIDI & instruments

| Capability | Phase | Risk | Notes |
|---|---|---|---|
| MIDI device I/O | 3 | L | |
| Note editor with velocity/CC lanes, quantise, humanise | 3 | M | |
| Groove templates (extract and apply timing/velocity feel) | 3 | M | |
| Step sequencer / drum patterns: swing, probability, Euclidean | 3 | L | |
| Scale mode | 3 | L | |
| Sampler, drum sampler, synth | 3 | M | Our own content only. |
| Melodic pattern sequencer | 7 | L | |
| Chord track driving tools | 7 | M | |
| Note transformations (strum, arpeggiate, ornament) | 7 | L | Strum is guitar-relevant. |
| MPE and MIDI 2.0 | 7 | M | |
| Constrained generators (rhythm, melody, chords) | 8 | M | Rule-based first. |
| Microtonal tuning (scale files) | 8 | M | |
| Articulation/expression maps | 8 | M | |
| Score/notation editor | 8 | H | Tab first. |
| Virtual accompanist players | — | H | Research only; clean training data required. |

## 7. Plugins & extensibility

| Capability | Phase | Risk | Notes |
|---|---|---|---|
| VST 3 effects | 1 | H | Decisive part of spike 0.1. |
| Out-of-process plugin scanning, blacklist | 1 | M | |
| Plugin editor windows embedded | 1 | H | Per-OS windowing edge cases. |
| Frozen-audio fallback for collaborators missing a plugin | 1 | M | |
| Sandboxing modes (in-process / shared / per-plugin process) | 2 | H | |
| Multi-output plugins, sidechain to plugins, bus layouts | 2–3 | M | |
| Plugin presets (native and our own) | 2 | L | |
| VST 3 instruments | 3 | M | |
| CLAP hosting | 3 | M | MIT SDK. |
| Audio Units hosting (macOS) | 3 | M | |
| ARA 2 hosting | 7 | H | |
| Intel-only plugins on Apple Silicon (bridge) | 0.7 decision | H | |
| User scripting (editor actions, MIDI) | 7 | M | Sandboxed. |
| Controller scripting / control surfaces | 7 | M | |
| Customisable key commands, command palette | 2 | L | |

## 8. Non-linear / performance

| Capability | Phase | Risk | Notes |
|---|---|---|---|
| Clip Grid: quantised launch, scenes, launch modes, follow actions | 6 | M | |
| Capture grid performance to timeline | 6 | L | |
| Pattern-based song building | 3 | L | |
| MIDI learn / controller mapping | 3 | L | |

## 9. Loop-station capabilities (Live Looper mode, Phase 5)

Generic capabilities found on multi-track loop stations, as a target list for
our own mode. Our version must show everything on a visible timeline and
mixer, not only behind physical buttons.

| Capability | Notes |
|---|---|
| Several stereo loop tracks with level/pan | Not limited to a fixed count. |
| Record → overdub → play on one control; configurable order (rec→dub or rec→play) | |
| Overdub modes: layer, replace, punch-replace | |
| Per-track undo/redo; bookmark a layer to return to | |
| Loop sync to a master length vs free length | |
| Tempo sync on/off; pitch follows tempo optional | |
| One-shot vs loop; reverse | |
| Quantised record/start/stop (beat, bar, loop end) | |
| Start modes: immediate, fade-in over N bars. Stop modes: immediate, fade-out, at loop end | |
| Play mode: multi vs single (one track at a time) | |
| Auto-record on input threshold | |
| Input effects (pre-record) and track effects (playback), several at once, in recallable banks | Any built-in or hosted plugin. |
| Master effects on the overall output | |
| Rhythm patterns with kits; optional intro, fill, ending; start/stop triggers | Our own patterns and samples. |
| Input mixer per source (gain, EQ, compression) | Covered by the general mixer. |
| Multiple outputs (e.g. separate monitor/PA feeds) | Covered by routing/monitor section. |
| Bounce playing loops into one track | |
| Saved phrase sets (memory slots) | |
| Footswitch/expression/MIDI control with press/hold/double-press actions | |
| Long recordings | Disk-bounded. |

## 10. Project, files & interchange

| Capability | Phase | Risk | Notes |
|---|---|---|---|
| Autosave, crash recovery, snapshots | 1 | L | |
| Import audio (WAV, AIFF, FLAC, MP3, Ogg/Opus) | 1–2 | L | |
| Export WAV + stems | 1 | L | |
| Export FLAC, MP3, Opus; stems by bus | 3 | L | |
| Templates | 2 | L | |
| Standard MIDI files | 3 | L | |
| DAWproject import/export (open, MIT spec) | 4 | M | |
| Track versions | 4 | M | |
| Content browser with tags, search, tempo-synced preview | 3 | L | |
| Video track | 8 | M | |

## 11. Collaboration (our differentiator, serverless)

| Capability | Phase | Risk | Notes |
|---|---|---|---|
| Bundle export/import with merge | 1 | L | |
| Shared-folder sync via any cloud-synced directory | 1 | M | Must avoid provider conflict copies. |
| Live session: teacher hosts, student joins by invite code | 1 | H | NAT traversal without a relay; spike 0.5. |
| Presence, follow mode | 1 | L | |
| Pinned comments and threads | 1 | L | |
| Lesson history | 3 | L | |
| Review mode (propose, diff, accept/reject) | 4 | M | |
| Live lesson call | 4 | M | |
| User-hosted relay for hard networks | 4 (open) | M | Only if we accept user-run infrastructure. |
| Real-time remote jamming | — | H | Limited by network latency; interval-delay mode possible later. |

---

## Research gaps

What we could and couldn't verify, as of October 2026:

- **Blocked sources.** This research environment could not reach several
  primary sources: the loop station's official Parameter Guide and Owner's
  Manual PDFs, one vendor's official feature page and release notes, and
  one review site. Findings for those products come from search-result
  summaries, retailer pages, secondary blogs and third-party manual mirrors.
- **Loop station details not confirmed:** the complete names of all input
  and track effect types (only counts and a partial list were confirmed),
  the full set of quantise options, and bounce behaviour.
- **DAW details from secondary sources only:** some edition-specific
  features (which tier includes what), stem-separation stem counts, and
  certain release-specific additions. These don't change our plan, since
  we specify capabilities generically.
- **Not researched yet:** other widely used DAWs beyond the four
  originally surveyed, mastering-specific tools, post-production/dialogue
  workflows, and education-focused music apps. These are worth surveying
  before Phase 3 (education) and Phase 8 (pro scale).
- **Ecosystem facts to re-verify before relying on them:** licences of
  every candidate library (verified by the CI licence check), Rust
  VST 3/CLAP host crate maturity (spike 0.1), the VST trademark/logo
  usage terms, and JUCE pricing tiers (fallback only).
