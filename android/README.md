# Phone remote (Android)

The app for ROADMAP 2.2: record and run the transport from a phone on
the same Wi-Fi as the desktop, with a timeline overview to move where
recording starts. The desktop side is `crates/remote`, and the app
speaks its protocol (`crates/remote/src/protocol.rs`).

- `core/`: plain Kotlin. The messages (`Protocol.kt`), the connection
  that keeps trying (`Connection.kt`) and the overview's sums
  (`Overview.kt`). Tested on any machine with a JDK 17 or newer:
  `./gradlew -PcoreOnly :core:test`. Its tests read the fixtures the
  desktop's tests write, so the two sides can't drift apart.
- `app/`: the Android app (Jetpack Compose). Needs the Android SDK;
  CI's `android` job builds it and uploads `daw-remote-android` (a
  debug package to sideload). Locally, open this folder in Android
  Studio, or run `./gradlew :app:assembleDebug` with `ANDROID_HOME`
  set.

## Pairing

On the desktop, Settings → Phone remote… → Show a code to pair a phone.
In the app, scan the code (or type the text under it). The code works
once; the desktop then remembers the phone until it is forgotten there.

## Dependencies

Apache-2.0 throughout: the Android and Compose libraries, Kotlin's
serialisation library, the camera library and the barcode library used
only to read the pairing code.
