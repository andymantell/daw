# 0013 — Hosting CLAP plugins: raw bindings, and ids made from plugin ids

- **Status:** Accepted (built at risk; the owner can still reverse it)
- **Date:** 2026-10-30
- **Deciders:** Built while the owner was away (roadmap 3.2, Phase 3 plan item A2); the roadmap already asked for CLAP hosting

## Context
Roadmap 3.2 asks for hosting plugins in the open CLAP format (MIT
headers) beside VST 3, effects first. Two things had to be chosen: which
Rust bindings to build on, and how a CLAP plugin is named in a project.
The app already names every plugin by a 16-byte class id (VST 3's), in
the project document (`Effect.class`), the plugin list, presets and the
helper process's messages. A CLAP plugin is named by a text id instead,
such as `org.example.reverb`.

## Options considered
Bindings:
1. **`clap-sys` 0.5.0**: the C headers as Rust types, nothing more.
   MIT or Apache-2.0, no dependencies, Rust 1.64. We write the host
   side ourselves, as we did for VST 3 on the raw `vst3` bindings.
2. **`clack-host` 0.2.0**: safe wrappers over the same headers, MIT or
   Apache-2.0, Rust 1.85. Less unsafe code of ours, but its own model
   of threads and lifetimes to fit around the helper process and the
   restart gate we already have, and a larger dependency to follow.

Ids:
1. **Make 16 bytes from the text id** (a 128-bit FNV-1a hash of `clap:`
   and the id). Nothing else changes: projects, presets, the list and
   the helper's messages carry 16 bytes as now. The bytes are the same
   on every computer, so a project opens wherever the plugin is
   installed. The text id itself is not in the project; the plugin's
   name and file path are, as for VST 3.
2. **Store the text id**, with a format field, in the project and
   everywhere else a class id goes now. Clearer to read, but it touches
   the document schema and every place that handles a plugin's id.

## Decision
`clap-sys` bindings, and 16-byte ids made from the text id
(`daw_plugins::clap::cid_of`). The module finds the plugin again by
hashing each id it offers. The file's extension (`.clap`) says which
format to load it as.

## Consequences
- A CLAP plugin works everywhere a VST 3 one does: the plugin list,
  in the app or in a helper process, the app's own presets, sidechains
  and extra outputs, restarts when its delay changes, and saving.
- A 128-bit hash can in principle give two plugins the same bytes; the
  odds are negligible. If we ever need the text id in the project
  (say, for a plugin missing everywhere), option 2 can be added beside
  this without breaking saved projects.
- Not built yet: CLAP instruments (plan B2, after notes land), the
  plugin's own preset lists (CLAP's preset discovery), mapping an older
  version to a newer one (CLAP has no standard way to declare it), and
  the plugin's window embedded on macOS and Linux (there it opens as a
  floating window of its own, if the plugin offers one).
