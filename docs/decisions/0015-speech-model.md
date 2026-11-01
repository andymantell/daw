# 0015 — A speech model for reading the words in vocals

- **Status:** Accepted
- **Date:** 2026-11-01
- **Deciders:** The owner, in the song test 3 thread ("Yes, local speech model") and in the lyrics thread (the small model, "let's try with the small model and see how it goes")

## Context
Song test 3 found that an assistant can't tell what a singer sings, so
it chose vocal chops by pitch and timing only. The wish was a readout of
a vocal clip's words with each word's time. The owner chose speech
recognition that runs on their own computer, with a model downloaded the
first time it is needed rather than shipped in the installer, under a
licence a GPL-3.0-or-later program can use ([0001](0001-project-licence.md)).

## Options considered
- **Whisper models run by whisper.cpp**, through the `whisper-rs`
  binding crate, which builds the C++ library from the source it carries
  (CMake and a C++ compiler, which CI already has for the Opus and ASIO
  builds). It gives each piece of a word a start and end time, so word
  times come with no extra work. Trained on a very wide mix of audio, it
  copes with singing better than speech-only models.
- **The same models in a pure Rust machine-learning library.** No C++
  build, but word times would have to be worked out by us from the
  model's attention, which is a project of its own.
- **A small speech-only recogniser** (tens of MB, word times built in).
  Smaller, but made for speech, and much weaker on sung words.

## Decision
`whisper-rs` 0.16 in a crate of its own (`daw-speech`). The app offers
three of the model's compressed files: base (60 MB), **small (190 MB,
the default)** and medium (539 MB). Each reads many languages and finds
which one it hears.

The model is downloaded once, after asking, from the model's official
page on huggingface.co, pinned to one revision, into the user's app data
folder. The file is checked against its size and SHA-256 before it is
used, and a file that doesn't match is thrown away.

### Licence check (read 2026-11-01)
- The models: the Whisper repository's README
  (https://github.com/openai/whisper) says "Whisper's code and model
  weights are released under the MIT License", and its `LICENSE` file is
  the MIT licence. The converted files' page
  (https://huggingface.co/ggerganov/whisper.cpp) lists its licence as MIT.
- whisper.cpp and ggml: MIT, from the `LICENSE` files in the source the
  binding crate builds.
- `whisper-rs` and `whisper-rs-sys`: Unlicense, from their crate records.
- The MIT licence and the Unlicense are both on the list of licences the
  dependency check allows (`deny.toml`), as GPLv3-compatible.

## Consequences
- The app's download grows by about 2 MB (the compiled library), not by
  the model.
- Builds take about a minute longer the first time, for the C++ library.
- Reading takes about 20 seconds for 90 seconds of audio with the small
  model on 4 cores. It runs off the UI thread.
- Sung words are sometimes misheard. Each word carries how sure the model
  is, and assistants are told to check by ear before cutting.
