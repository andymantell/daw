# Feature gaps

Where the app stood on 30 October 2026 against the feature sets of
widely used desktop DAWs, written **in generic terms** as
[CLEAN_ROOM.md](../CLEAN_ROOM.md) requires. It names no product. It
adds to [CAPABILITY_LANDSCAPE.md](CAPABILITY_LANDSCAPE.md), which maps
capabilities to roadmap phases, and records what that survey left out.

How this was made: we read the official product pages of eight major
DAWs, then the full contents lists of their official manuals (two to
four headings deep, not every page of text), on 30 October 2026. Each
feature found was searched for in the roadmap, the review queue and the
code. "Found in" counts how many of the eight manuals cover it. "Worth
it" is a judgement for a guitar-lesson app growing into a full DAW, not
a measurement, and it is still to be agreed with the owner. A feature
listed here could still exist in our code under a name the search
missed.

Everything marked built was built at risk: tested in the cloud, not yet
on the owner's hardware (see [REVIEW_QUEUE.md](../REVIEW_QUEUE.md)).

## Summary

- **Level, built at risk:** recording with punch, loop takes and comping
  with several comps; keeping audio played without Record on; a monitor
  section with headphone mixes and talkback; buses, sends, control groups
  and delay compensation; automation modes; VST 3 and CLAP effects and
  instruments in crash-safe helper processes; effect containers and
  layered instruments; drum machine, sampler, slicer, synth and step
  sequencer; per-note expression; tempo and metre changes; freeze and
  render in place; compressed export formats; video clips with timecode
  and hit points; loudness meters; panes, layouts and extra windows.
- **Planned but not built:** audio that follows tempo, pitch editing and
  real-time pitch correction, stem separation, a clip launcher grid,
  chords and a chord track, modulators, plugin-based audio editing
  (ARA), Audio Units and macOS, surround and immersive mixing, a
  mastering assistant, MIDI clock and external hardware, live sessions,
  the agent interface (2.11), and keeping recent MIDI without recording
  (3.1).
- **Ahead:** serverless collaboration with merging, time-pinned comments
  and per-person undo; practice tools (speed and pitch, tempo trainer,
  tuning presets, practice mute); lessons (notes, tab blocks, practice
  log, assignments); tab and drum notation lanes under the tracks; an
  electronic drum kit learn tool; screen-reader access to the timeline.
  None of the product pages read lists these.
- **Not on the roadmap:** about 70 features, below. The ones suggested
  for the plan first are marked **High**.

## Not on the roadmap

### Recording

| Gap | Found in | Worth it |
| --- | --- | --- |
| Separate buffers: live inputs and monitoring on a small buffer, the rest of the mix on a large one | 2 | **High**: monitoring through an amp sim is the core use |
| Record a track's or bus's output into a new track | 4 | Medium |
| Record with the effects printed into the take | 2 | Medium |
| Step entry of notes, one at a time, from a keyboard | 6 | Medium |
| Quantise notes as they are recorded | 3 | Medium |
| The computer keyboard or an on-screen keyboard as a note input | 2 | Medium |
| Wait for the first note before recording starts | 1 | Low |
| Record-safe and lock-record to protect tracks | 2 | Low |
| Colour, mark or rate takes while recording | 2 | Low |
| Record the computer's own sound output | 1 | Low |

### Editing and arranging

| Gap | Found in | Worth it |
| --- | --- | --- |
| Arranger track: play sections in any order, repeat them, then flatten | 3 | **High**: song structure and practice |
| Insert or delete time across every track, tempo change and marker | 4 | **High** |
| Linked copies of audio and note clips (patterns already have them) | 2 | Medium |
| Glue or join clips into one | 4 | Medium |
| Remove silent passages automatically | 4 | Medium |
| Effects on a single clip rather than the whole track | 2 | Medium |
| Align the timing of a double-tracked part to a guide | 2 | Medium |
| Replace or double drum hits from audio with samples | 2 | Medium |
| Phase-align several mics on one source | 1 | Medium: multi-mic drums |
| An undo history list to jump back several steps | 4 | Medium (close to Phase 4 activity history) |
| Lock clips or tracks against accidental edits | 4 | Low |
| Place a clip at an exact timecode | 1 | Low |
| Snap edits to zero crossings | 1 | Low |
| Destructive sample tools: DC offset removal, polarity flip, statistics | 2 | Low |
| Generate harmony voices from a sung line | 1 | Low |
| Speech transcription and editing by word | 1 | Low |
| Rename many tracks or clips at once | 2 | Low |

### Notes and MIDI

| Gap | Found in | Worth it |
| --- | --- | --- |
| Real-time note effects: arpeggiator, chord, note length, velocity, randomise, scale | 8 | **High** once instruments are in use |
| A track of key changes that transposes playback | 2 | Medium |
| Lyrics on a track and under notation | 3 | Medium: song recreation |
| Mute single notes (whole note clips can be muted already) | 3 | Low |
| Event list editor, including program changes | 4 | Low |
| Rule-based batch edits ("every note above velocity 100 on beat 1") | 2 | Low: the agent interface could cover it |
| Split a part by pitch or channel onto separate tracks | 2 | Low |
| Generative note tools (rhythm, shape, stacked chords) | 2 | Low (close to Phase 7 generators) |

### Mixing and effects

| Gap | Found in | Worth it |
| --- | --- | --- |
| Save a track or its effect chain as a preset and reuse it | 5 | **High**: guitar tones |
| A few macro knobs per track, mapped to any settings | 4 | Medium |
| Mixer scenes that recall fader and effect settings | 2 | Medium |
| Solo modes: solo-safe, and a listen bus that leaves the mix alone | 3 | Medium |
| Turn a track off to save processing but keep it in the project | 3 | Medium |
| A container that switches between chains (e.g. clean and driven amp channels) | 2 | Medium: also suits stage rig mode (5.6) |
| Dither and noise shaping on export | 3 | Medium |
| Noise and voice denoise tools | 2 | Medium: home recording |
| Per-track time offset | 3 | Low |
| Pan law choice and dual panning | 2 | Low |
| Notes on each mixer channel | 4 | Low |
| More effects: vocoder, pitch shifter, frequency shifter, stutter, grain delay, linear-phase EQ, matching EQ | 8 | Low |
| More instruments: FM, wavetable, organ, electric piano, physical models | 8 | Low (sound content waits on the owner) |
| Test-tone generator | 2 | Low |

### Projects and files

| Gap | Found in | Worth it |
| --- | --- | --- |
| Import tracks, markers or settings from another project | 5 | **High**: a teacher reuses lesson parts |
| Several projects open at once, dragging between them | 2 | Medium |
| Export chosen tracks or a time range as a new project | 2 | Medium |
| Find and remove audio no longer used | 2 | Medium: takes pile up |
| Export each section or marker range as its own file, and queue exports | 2 | Medium: a practice file per section |
| Export video with the new mix | 5 | Medium |
| Start with every plugin switched off after a crash | 2 | Medium |
| Interchange formats used by video editors | 4 | Low |
| Batch file converter | 2 | Low |
| Album sequencing and mastering page | 1 | Low |

### Workflow, control and sync

| Gap | Found in | Worth it |
| --- | --- | --- |
| Chain several commands into one key (macros) | 2 | Medium: cheap on the shared command layer |
| Hosting LV2 plugins | 1 | Medium on Linux |
| Custom toolbars, menus and mouse behaviour | 2 | Low |
| A translated interface | 2 | Low |
| Hints showing what each modifier key does | 1 | Low |
| Tablet and touch editing mode | 2 | Low |
| Hosting VST 2 plugins (now only mapped to newer versions, 2.4) | 1 | Low |
| Running as a plugin inside another host | 1 | Low |
| CV and clock out to modular hardware | 2 | Low |
| Control by network messages (OSC) | 2 | Low |
| Linking computers to share the processing | 2 | Low |
| A modular sound-design environment | 2 | Low |
| Virtual accompanist players | 2 | Already "research only" in the landscape |
| An online sound library and cloud backup | 3 | Clashes with "no server we operate" |

## Planned, and standard elsewhere

These are on the roadmap but not built, and appear in at least three of
the eight manuals: keeping recent MIDI without recording (3.1, at least
five), tap tempo (Phase 5), tempo sync over the network (5.6), MIDI
clock and timecode (Phase 8), audio to notes (Phase 7), chord tracks
(Phase 7), clip launching (Phase 6) and control-surface support
(Phase 7).

## What users ask for

A second pass, also on 30 October 2026, read what users ask for rather
than what vendors ship: the top requests on two vendors' public
feature-request forums, a user wishlist thread, four "what my DAW is
missing" articles, one page each of review cons for two DAWs, three
sources on blind musicians' needs and two on teachers' needs. Several
other forums could not be reached (login walls, blocks or human checks).
It is a day's sample, not a survey.

Most of the loudest requests are already built or planned: Linux, crash-
safe plugins, control by AI or script, audio that follows tempo, song
sections, track presets, a clip launcher and network tempo sync. These
were not on the roadmap or in the tables above:

| Wish | Worth it |
| --- | --- |
| A simpler mode for beginners that hides advanced features (the most common complaint in the review cons read) | **High**: students |
| Guided audio setup, without driver trouble or latency surprises | **High** |
| Accessibility across the whole app, not just the timeline: no mouse needed, controls that stay put, accessible plugins, and ways to hear EQ, pan and automation | **High** |
| A fretboard view of notes, scales, modes and arpeggios | Medium: guitar lessons |
| Recording at a slower speed without changing the song's tempo | Medium: practising hard passages |
| Plugin windows: tabs, following the selected track, renaming, CPU per plugin, a list of plugins a project uses | Medium |
| Stop the mouse wheel moving faders by accident | Low |
| A polarity button on each input | Low |
| Jump to the next attack in audio | Low |
| A seamless loop export | Low |
| Choosing which C is middle C, and custom note names | Low |
| Pinning tracks at the top; undo that keeps the selection; search in settings | Low |
| A phone app as a remote; media bins; a plain-language export summary | Low |

## Next step

The owner decides which gaps join the roadmap. Until then this page is
reference only.
