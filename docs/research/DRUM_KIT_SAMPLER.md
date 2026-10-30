# Research: electronic drum kit instrument

Checked 2026-10-30, for a feature request: an instrument that understands
the pad mapping of as many electronic drum kits as possible and triggers
drum samples, plus a source of samples. The owner plays a mid-range
electronic kit with a five-piece pad set.

## Findings

### How kits send their hits

- The owner's kit module sends MIDI over USB (and a DIN MIDI out), on
  channel 10 by default. Its manual says the MIDI note number is set per
  pad in the setup menu, with separate notes for snare head and rim and
  for cymbal bow and edge, and warns that duplicated note numbers play one
  sound. **The manual gives no default note table**, and it does not say
  how the hi-hat pedal is sent. *(Source: the module's user manual, read
  through a manual-hosting site on 2026-10-30; the maker's own site was
  not consulted.)*
- **Consequence:** a profile library alone cannot cover every kit, because
  owners change notes and some makers publish no table. A **learn wizard**
  that asks for each pad and zone in turn, and watches what arrives
  (notes, and a controller for the hi-hat pedal), works for any kit and
  doubles as the way to build and share profiles.
- The General MIDI percussion key map (notes 35 to 81: kick, snare,
  toms, hi-hats, cymbals and percussion) is the common default that most
  kits can be set to. *(The exact table is to be taken from the MIDI
  Association's published General MIDI specification before
  implementation; only secondary copies were found in this pass.)*
- Hi-hat pedal position is commonly sent as a continuous controller;
  which controller number, and whether position is sent before the note,
  varies by maker. Treat it as per-profile data learned by the wizard.

### Samples under licences we can bundle

- **AVL Drumkits:** recorded acoustic kits plus hand percussion, 5 to 10
  velocity layers per piece, provided in SFZ, SoundFont and an open plugin
  format. Licensed **CC BY-SA 3.0**; the page says music made with them
  needs no attribution and share-alike applies only if the samples or the
  library formats are modified. *(Source: the project's own page, read
  2026-10-30.)*
- **Salamander Drumkit:** an open-source SFZ kit (kick, rack and floor
  toms, two snares, several cymbals, hi-hat), few velocity layers but many
  round-robin samples per layer (for example 40 closed hi-hat samples).
  Licensed **CC BY-SA 3.0**; its README says share-alike applies only if
  the samples themselves are modified. *(Source: the kit's repository
  README, read 2026-10-30.)*
- **Consequence:** both can ship with the app with credits, the same way
  the reverb rooms do (decision 0011), as long as we redistribute the
  samples unmodified or under the same licence. Converting the format
  (for example to our own container) should keep the samples byte-for-byte
  or be treated as a modification under the same licence. Check the exact
  licence text of each set before bundling.
- **SFZ** is an open, text-based sampler format used by both sets and by
  many other free instruments, so supporting it gives users a large pool
  of kits.

## Learning once, shipping by default

Follow-up request (2026-10-30): a small tool that learns the owner's kit
mapping, with the result built into the app by default, so nothing has to
be trained again on a new computer.

- **Where the mapping lives.** The note number for each pad is stored in
  the kit module's own settings, not on the computer (the module's manual
  sets notes per pad in its setup menu). So a profile learned once
  describes that module wherever it is plugged in; a new computer only
  needs the profile file, not a new learning session.
- **Profile file.** The learn tool writes a plain text data file: kit
  name, the MIDI port name it was learned from, channel, and for each
  pad and zone the note number, plus the hi-hat pedal controller number
  and its closed-to-open range and any choke message seen. Profiles are
  stored with the app's user settings (not inside a project), can be
  exported and imported as files, and the bundled set lives in the
  repository so it is part of every build.
- **Bundling the owner's kit.** Once the owner runs the tool on their
  kit, the resulting file is committed as the first bundled profile. If
  their pads still carry the factory notes, that profile is also the
  default for every other owner of the same module; the file records
  whether the notes were factory or changed, as far as the owner can
  tell. *(The maker's official default table was not found in this pass,
  so the learned file is the best source we have.)*
- **Recognising a kit.** The app can match a bundled profile to a
  connected kit by its MIDI port name and offer it with one click; a
  learned profile also stores that name. Port names come from the
  operating system and can differ between platforms, so the match is a
  suggestion, not a rule.
- **Order of work.** The tool needs only MIDI input, which exists (3.1),
  so it can be built now as a small, separate item: a window under the
  MIDI settings that prompts for each pad and zone, shows what arrived,
  and saves the profile. The instrument itself (samples, zones, hi-hat
  modelling) stays a late Phase 3 item and reads the same profiles.

## Design notes

- Kit profiles and drum maps are the same data as the drum staff's drum
  map (roadmap 3.8), so a learned profile also makes notation correct.
- Latency matters more for drums than anything else; the instrument sits
  on the same low-latency monitoring path as audio input.

## Not researched yet

- The maker's official default note table and hi-hat controller for the
  owner's kit, from the maker's site.
- Which other makers publish default maps, to seed the profile library.
- Whether any set has a small, dry studio kit suited to lessons, and the
  download routes from the build machines (see the reverb-room notes).
