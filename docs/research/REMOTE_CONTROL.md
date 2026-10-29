# Research: phone remote control

Checked 2026-10-29, for a feature request: an Android app with transport and
record controls for use away from the computer (for example at a drum kit),
plus a timeline overview to move the recording start point.

## Findings

- **A web page served by the desktop can't keep the phone awake.** The
  Screen Wake Lock API works only in secure contexts (HTTPS). Origins count
  as potentially trustworthy only with an `https`, `wss` or `file` scheme,
  or on loopback (`127.0.0.0/8`, `::1`, `localhost`). A page on plain HTTP
  from a local-network address therefore isn't a secure context. *(Source:
  MDN, "Screen Wake Lock API" and "Secure contexts", read 2026-10-29.)*
  Serving HTTPS on the local network would need a certificate the phone
  trusts, which means browser warnings or manual certificate setup.
- **A native Android app can stay awake simply.** An activity sets
  `FLAG_KEEP_SCREEN_ON` (or `android:keepScreenOn`). The official guide
  shows no permission for this. The system lets the screen turn off
  normally once the app is in the background. *(Source: Android Developers,
  "Keep the screen on", read 2026-10-29.)*
- **Decision for the plan:** a native Android app in Phase 2. A served web
  page stays as the fallback for other devices in Phase 5.

## Design notes

- The desktop is the source of truth. The phone sends commands and shows
  state, and a dropped connection never affects a recording in progress.
- Pairing by QR code holds the desktop's local address and a one-time
  secret, so other devices on the same network can't take control.
- One control protocol serves the Android app, the Phase 5 looper pages and
  the served web page.

## Settled since

- **Language and toolkit** (2026-10-29): Kotlin with the platform's
  declarative UI toolkit; the protocol is written on both sides and kept
  in step by shared fixtures. See
  [decision 0010](../decisions/0010-phone-remote-app.md).
- **Distribution**, first step: a debug package built by CI to sideload.
  A signed package on our releases, or a store listing, is the owner's
  call (review queue 2.2).

## Not researched yet

- Discovering the desktop on the local network without scanning a code
  (and so surviving a change of the desktop's address).
