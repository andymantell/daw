# Research: platforms and distribution (roadmap 0.7)

Checked 2026-10-27. This covers which operating system versions to support,
Apple Silicon and Intel Macs, code signing, updates with no server we
operate, crash reports, and Linux packaging. Items marked **Owner decision**
are also listed in [review-queue/0.7-platform.md](../review-queue/0.7-platform.md).

## Operating system versions

### Windows

- Windows 10 support ended on 14 October 2025. Paid consumer Extended
  Security Updates run until 12 October 2027. *(Source: the OS vendor's
  support article KB5056949, read 2026-10-27.)*
- Rust's 64-bit Windows target needs Windows 10 or later and is Tier 1. The
  64-bit ARM Windows target is also Tier 1. *(Source: the Rust platform
  support page.)*
- **Proposal:** support Windows 10 22H2 and Windows 11, on x64 first.
  Windows on ARM follows once a test machine is available. Many school and
  home PCs still run Windows 10, so dropping it now would cut out students.
  Review this when consumer security updates end in October 2027.

### macOS

- macOS 26 is the last release for Intel Macs. macOS 27 (announced June
  2026) runs on Apple Silicon only. Intel Macs on macOS 26 get security
  updates for about three years. *(Source: reporting that quotes the OS
  vendor's announcement. I could not find the vendor's own page for this,
  so it should be checked again before release.)*
- Rosetta (which runs Intel code on Apple Silicon) stays general-purpose
  through macOS 27. After that, the vendor keeps only "a subset of Rosetta
  functionality aimed at supporting older unmaintained gaming titles".
  *(Same source.)*
- Rust's Apple Silicon target needs macOS 11 or later and is Tier 1. The
  Intel macOS target needs 10.12 or later and is Tier 2. *(Source: the Rust
  platform support page.)*
- **Proposal:** ship a universal app (Apple Silicon and Intel in one
  bundle), with macOS 13 or later as the minimum. This minimum is a guess
  pending real testing; the audio and windowing libraries may need a
  higher one. Drop Intel when macOS 26 security updates end.
- **No macOS test machine exists** (see the test-rig note in project
  memory). Everything on macOS stays CI-built and untested until one is
  found.

### Intel-only plugins on Apple Silicon

- A native Apple Silicon app cannot load Intel-only plugin binaries into
  its own process.
- The out-of-process plugin host (spike B, `plugin-host oop`) already puts
  each plugin in its own process. Building that helper as an Intel binary,
  run under Rosetta, would let Intel-only plugins load. The cost is the
  extra round trip already measured for the out-of-process host.
- After macOS 27, Rosetta may no longer run general apps (see above), so
  this bridge has a limited life. **Proposal:** support Intel-only plugins
  through the Rosetta helper only while the OS supports it. Say clearly in
  the plugin list that such plugins will stop working, and do not build
  anything more on it.

### Linux

- Rust's 64-bit Linux target needs kernel 3.2+ and glibc 2.17+ (Tier 1).
- **Proposal:** test on the current Ubuntu LTS. Build on an older base so
  the app runs on a wide range of distributions.

## Code signing

### Windows

From the OS vendor's code-signing options page (read 2026-10-27):

| Route | Cost | Notes |
|---|---|---|
| The OS vendor's cloud signing service | About $9.99 a month | Organisations in the US, Canada, EU and UK. **Individuals only in the US and Canada**, so an individual in the UK cannot use it. No instant reputation with the download filter. |
| An OV certificate from a certificate authority | About $150–300 a year | Since June 2023 the key must be on a hardware token or HSM, so signing in CI needs a cloud HSM or a hosted signing service. |
| An EV certificate | More than OV | Lost its instant download-filter bypass in 2024, so it no longer has a clear advantage for us. |
| A free signing service for open-source projects | Free | OV-level signing done by a foundation that sponsors open-source projects. |

The free open-source signing service's terms (read 2026-10-27) include:

- an OSI-approved licence, with no commercial dual licensing (GPLv3 fits);
- automated, verifiable builds in CI;
- a project that is already released;
- manual approval of each release;
- no proprietary components, with system libraries excepted;
- the signing team maintains the source;
- multi-factor authentication.

**Open question:** we use the ASIO SDK under its GPLv3 option. I don't know
whether the service would count that as a proprietary component. Ask the
service before applying.

Unsigned builds still run on Windows. The download filter warns users until
the file builds up reputation, and some school-managed PCs block unsigned
apps entirely.

**Proposal:** ship unsigned builds for testers now. Apply to the free
open-source signing service after the first public release. Fall back to
an OV certificate with a cloud HSM if the application is refused.
**Owner decision.**

### macOS

- The Apple Developer Program costs 99 USD per membership year, with prices
  varying by region. *(Source: the program's enrolment page, read
  2026-10-27.)*
- Fee waivers are for nonprofits, accredited schools and government bodies
  that are legal entities only. Individuals, sole traders and one-person
  businesses are not eligible, and being open source alone does not
  qualify. *(Source: the program's fee-waivers help page.)*
- Apps distributed outside the App Store are signed with a Developer ID
  certificate, which only the account holder can create. Notarisation
  uploads the signed app to the vendor's notary service, which checks it
  and issues a ticket that is stapled to the app. *(Source: the vendor's
  Developer ID page.)* The vendor's page calls notarisation optional. In
  practice, current macOS versions warn hard about un-notarised apps, and
  since macOS 15 the old Control-click way of opening one is gone: users
  must allow it in System Settings → Privacy & Security. *(Source: tech
  press, not the vendor; noted as such.)* For students and parents, that
  is a real barrier.
- From general knowledge, not checked on a vendor page today: notarisation
  needs the hardened runtime. Loading third-party plugins in-process then
  needs the "disable library validation" entitlement. The out-of-process
  plugin helper could carry that entitlement on its own, so the main app
  does not need it.
- **Proposal:** join the program (99 USD a year) before the first macOS
  release, and notarise every release in CI. No macOS release goes out
  until then. **Owner decision** (it is a yearly cost in Andy's name, or a
  legal entity's).

### Linux

No signing scheme is needed for the user to run the app. Releases carry a
signature file (see updates below).

## Updates with no server we operate

- **Proposal:** release files hosted on the code host's release pages.
  The app checks the latest-release feed at most once a day, and only when
  the user has turned this on. It shows a "new version available" note
  with a link. It does not download or install anything by itself in
  version 1.
- Every release ships a detached signature, made with a key that only the
  release CI holds. The app checks this signature before suggesting an
  update, so a hijacked release page cannot push a bad build. Updating
  itself in place can come later. It is harder on Windows (files in use)
  and on macOS (keeping the signature valid).
- Privacy: the check sends no identifiers, so the code host sees only an
  IP address and a user agent. The setting should say this, given that
  users may be children (roadmap 1.8).

## Crash reports

- **Panics** (Rust errors): a panic hook writes a report file (version, OS,
  backtrace) to the app's local data folder.
- **Native crashes** (most likely from plugins): a crash-handler library
  can run code on a fatal signal or exception on Windows, Linux and macOS.
  The one checked here is MIT/Apache-2.0. Writing a full crash dump safely
  usually means a second, watching process. I have not yet checked how the
  matching dump-writer library works, so that is still open.
- Plugins already run in their own processes (spike B), so a plugin crash
  is reported by the host as "plugin X stopped" and does not take the app
  down. That covers the most common crash cause without any dump code.
- On next start, the app offers to open the folder holding the report, so
  the user can attach it to a bug report. Nothing is sent automatically,
  and there is no telemetry server.

## Linux packaging

- **AppImage:** one file, no install, no sandbox, so plugins in the usual
  folders and the audio system just work. It needs FUSE 2. Ubuntu 22.04+
  and Debian ship only FUSE 3 by default, so users must install
  `libfuse2` (`libfuse2t64` on Ubuntu 24.04). Running with
  `--appimage-extract-and-run` works without FUSE. *(Source: the format's
  own troubleshooting page.)*
- **The main Flatpak store:** its rules say apps that "rely on host
  components … for core functionality" are not accepted, with possible
  exceptions, and static permissions must be kept to a minimum. *(Source:
  the store's requirements page.)* A DAW that loads plugins installed on
  the host needs broad file access and probably a shared plugin extension.
  Other audio apps are on the store, so this is possible, but it needs
  talking to the reviewers first.
- **Proposal:** AppImage first, plus a plain `.tar.gz`. Look at Flatpak
  after version 1, talking to the store's reviewers before submitting.
  **Owner decision.**

## Still to check

- The vendor's own pages on the end of Intel Macs and on Rosetta (only
  press reports so far).
- Whether the hardened runtime is required for notarisation, and the exact
  plugin-loading entitlements, from the vendor's documentation.
- The ASIO question with the open-source signing service.
- How out-of-process crash-dump writing works with the crash-handler
  library.
- The real minimum macOS version, once the audio and window libraries are
  pinned.
