# 0010 — Phone remote: how the Android app is built

- **Status:** Accepted 2026-10-29 (by the thread that built it), subject
  to the owner's first run on a phone
- **Date:** 2026-10-29
- **Deciders:** project owner (to confirm)

## Context
ROADMAP 2.2 asks for an Android app that records and runs the transport
from across the room, with a timeline overview, level meters, pairing by
a code on the desktop's screen, and a desktop that stays in charge.
[research/REMOTE_CONTROL.md](../research/REMOTE_CONTROL.md) settled on a
native app (a served web page can't keep the screen awake) and left open
which language and toolkit to use, and whether the app can share Rust
code with the desktop.

The build machines for this project have no Android SDK (its download
host is not reachable from them), so whatever is chosen, the app itself
compiles only in CI and on the owner's machine.

## Options considered
1. **Kotlin with the platform's declarative UI toolkit, the protocol
   written again in Kotlin.** The app is thin: buttons, meters, one
   drawn overview, a camera view for the code. Everything that makes it
   an app (keep the screen on, haptics, the camera, permissions,
   lifecycle) is a platform call. Sharing is by contract: the desktop's
   tests write example messages to `crates/remote/tests/fixtures/`, and
   the app's tests read the same files.
2. **Rust on the phone too, with the desktop's UI toolkit**, through its
   mobile support and a native-development-kit build. Shares the
   protocol crate. Needs the native toolkit, a cross-compiler, a thin
   platform shell anyway, and a bridge call for every platform feature
   above. Two toolchains in CI instead of one, and the UI toolkit's
   mobile text input and camera support are young.
3. **Kotlin UI over a Rust core through a binding generator.** The most
   machinery of the three, to share a crate of a few hundred lines.

## Decision
Option 1. The protocol is small enough that writing it twice costs less
than building and maintaining a cross-compiling toolchain, and the
fixture contract keeps the two copies honest in CI. The app's Kotlin is
split so that everything without the platform in it (`android/core`:
messages, the connection that keeps trying, the overview's sums) builds
and tests on any machine with a JDK, which is how it was developed and
tested here.

## Consequences
- Easier: one Gradle build in CI, standard tooling for anyone who has
  written an Android app, and the phone side of the protocol is readable
  without Rust.
- Harder: a protocol change is made twice, and forgetting the second
  copy fails the app's tests rather than the compiler. The `VERSION`
  constant exists in both and must move together.
- The app has not run on a phone yet (none on the build machines). The
  first run is in the review queue; the camera scanner is the part most
  likely to need a fix after it.
- Later remotes (the looper pages, a served web page) speak the same
  protocol; the web page would reuse the fixtures the same way.

## Sources
- Android Developers, "Keep the screen on" and "Request camera
  permission", read 2026-10-29.
- Our own build: `android/core` tested here against the desktop's
  fixtures (`crates/remote/tests/fixtures.rs`).
