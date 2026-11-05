# Owner answers

Answers the owner gave to the open decisions in this folder, newest
first. Each answer is also ticked in its feature's file.

## 2026-11-05: unsigned macOS builds

No paid developer program for now. macOS builds go on the release page
unsigned, as Windows ones do, with steps for allowing the app on first
launch (System Settings → Privacy & Security → Open Anyway). Not built
yet: the release workflow has no macOS job. The Intel-only plugins
question waits until a Mac build is actually tested.

## 2026-11-05: Windows builds stay unsigned for now

No code signing, and no application to a signing service, until the
owner asks (`0.7-platform.md`). Windows shows its unknown-publisher
warning on the downloaded app meanwhile.

## 2026-11-05: decision 0009 (notation formats) accepted

Accepted as built: MusicXML, standard MIDI files, plain-text tab and
PDF. The proprietary tab-editor formats stay excluded; the deferred
file-format study idea stays deferred.

## 2026-11-05: recorded cabinets ship too

The guitar amp also ships a few recorded cabinet impulse responses
under licences that allow it, credited like the recorded rooms, beside
the five made in code (`2.5-amp-and-cabinet.md`). Each file's licence is
checked before it is added. This is the one exception to "no sample
files". Not built yet: after the polish pass.

## 2026-11-05: sounds are made in code, and there are more of them

No sample files ship with the built-in instruments
(`3.4-built-in-instruments.md`). Instead, the sounds made in code are
widened so every instrument has a useful starting set: drum kits, keys,
basses, pads, and each synth's own sounds. The four Patch Twenty sounds
ship under the names they have now. Not built yet: after the polish pass.

## 2026-11-05: Empty and Guitar lesson ship, with effects

Empty and Guitar lesson ship; Band demo is removed
(`2.8-templates-and-changes.md`). The Guitar lesson tracks get a light
EQ and a little reverb from the built-in effects. Not built yet: after
the polish pass.

## 2026-11-05: plugins run in the shared helper by default

Kept as built (`2.4-plugin-process.md`): new effects run in the shared
helper process, trading one audio buffer of delay for crash safety. "In
the app" stays a per-plugin choice in Settings → Plugins.

## 2026-11-05: deleted lesson notes and tab are wiped

Options (a) and (b) of `1.7-lesson-notes.md`: deleting a tab block wipes
it from the saved file, its backup and bundles, as comments are, and
deleted or rewritten lesson-note text is blanked from the history too.
Not built yet: it waits until feature work resumes after the polish
pass. A snapshot taken before a deletion then restores without the
deleted words (`1.8-snapshots.md`).

## 2026-11-05: built-as-is defaults accepted

The owner accepted every "built like this, say if you want it changed"
item under the decision headings in this folder (203 items), so they are
ticked. Any of them can still be changed later. Checks on the PC and
listening checks are not affected and stay open.

Decisions 0012 (amp capture player), 0013 (CLAP hosting) and 0014
(export encoders), built at risk, are confirmed.

Still open and being asked one at a time: Intel-only plugins on Apple Silicon (waits for a Mac
build), and public STUN servers. The speed and pitch choice (0.6) and decisions 0003 and 0007
wait for listening and checks on the PC.
