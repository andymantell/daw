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
