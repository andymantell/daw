# Releasing

How installers are built and published (roadmap 1.8). This follows the
0.7 proposals in [research/PLATFORM_DISTRIBUTION.md](research/PLATFORM_DISTRIBUTION.md),
which are still waiting for the owner's sign-off in
[REVIEW_QUEUE.md](REVIEW_QUEUE.md); what changes if they do is listed at
the end.

## What a release contains

| File | For |
|---|---|
| `daw-<version>-windows-x64-setup.exe` | Windows 10 22H2 or 11, 64-bit. Installs for the current user into `%LOCALAPPDATA%\Programs\DAW` with a Start menu entry; no administrator rights needed. Uninstall from Settings → Apps. Not signed yet, so Windows warns before running it. |
| `daw-<version>-windows-x64.zip` | The same program to unpack and run without installing. |
| `daw-<version>-linux-x86_64.AppImage` | Linux, 64-bit: one file; make it executable and run it. Needs the system's ALSA library, as any audio app does. |
| `daw-<version>-linux-x86_64.tar.gz` | The Linux program with a desktop entry and icon, to unpack anywhere. |
| `latest.txt` | The version and every file's SHA-256 checksum. |
| `latest.txt.minisig` | The release workflow's signature over `latest.txt`. |

There is no macOS release until the Apple developer decision is made.

## Making one

1. Set the version in the root `Cargo.toml` (`[workspace.package]`),
   commit it on `main`, and wait for CI.
2. Tag that commit with the same version and push the tag:
   `git tag v0.1.0 && git push origin v0.1.0`.
3. The **release** workflow (`.github/workflows/release.yml`) builds
   everything, signs `latest.txt`, and makes a **draft** release with the
   files attached. The workflow refuses a tag that does not match the
   version in `Cargo.toml`. A version with a hyphen (`0.1.0-beta.1`) is
   marked as a pre-release.
4. Check the draft on the repository's Releases page, then publish it by
   hand. Nothing is public until then, and the app's update check only
   sees the newest published release that is not a pre-release.

The same workflow runs on pull requests that change `packaging/` or the
workflow itself, and can be run by hand from the Actions tab: it then
builds and packages everything and keeps the files as the
`release-files` artifact, without making a release.

## The signing key (one-time setup by the owner)

The app only tells the user about a new version after checking
`latest.txt` against the public key built into it
(`packaging/release-key.pub`). Until that file holds a key, builds cannot
check for updates at all, and say so in Help → Check for updates.

The signatures use the minisign format (Ed25519). To set it up once:

1. Install the `minisign` tool (Windows builds are on its project page;
   on Linux it is packaged as `minisign`).
2. Make a key pair with no password, since only the workflow will use it:
   `minisign -G -W -p release-key.pub -s release-key.sec`
3. In the repository's **Settings → Secrets and variables → Actions**, add
   a secret named `RELEASE_SIGNING_KEY` whose value is the whole content
   of `release-key.sec`.
4. Replace `packaging/release-key.pub` with the new `release-key.pub` and
   commit it (or paste it into a thread). The public key is not secret.
5. Delete `release-key.sec` from your computer, or keep it somewhere
   safe offline. Anyone with it can sign releases the app will trust.

The workflow checks each signature against the committed public key, so
a mismatch fails the release rather than shipping something no app can
check. Without the secret, the files are still built but `latest.txt`
is unsigned, and the draft's notes say the update check will ignore it.

If the key is ever lost or leaked, make a new pair and replace both.
Apps already installed only trust the old key, so their users need to
download the next version by hand once.

## Update check

Off until the user turns it on. At the first start of a build that has
a key, the app asks once, in plain words, saying that checking lets
GitHub see the computer's internet address. When on, it fetches
`releases/latest/download/latest.txt` and its signature at most once a
day, checks the signature, and if the version is newer shows a note
with a link to the release page. It sends nothing else (the request
names only the app and its version) and downloads or installs nothing.
Help → Check for updates turns it on or off and checks on demand.

## Crash reports

If the app hits an internal error (a Rust panic), it writes a text
report into `%LOCALAPPDATA%\DAW\Crash reports` on Windows, or
`~/.local/state/DAW/Crash reports` on Linux: version, system, where it
happened and a backtrace, with the user's name, home folder and any file
path in the error message taken out. At the next start a note says a
report was saved; Help → Crash reports shows each one's exact text and
opens the folder. Nothing is sent. The 20 newest are kept.

Not yet covered: crashes in native code, such as a plugin crashing while
playing. Plugin scanning already runs in its own process, so a plugin
that crashes while being scanned is reported without taking the app down.

## At risk on the 0.7 decisions

Built ahead of the owner's sign-off on the 0.7 proposals. If they
change:

- **Windows signing:** signing would be added to the workflow; the
  installer itself stays.
- **Minimum OS versions:** the Linux build runs on the oldest supported
  Ubuntu LTS (22.04) to keep the C library old enough; a different
  minimum changes the runner.
- **Updates:** a different scheme (for example self-install) would
  replace `crates/app/src/update.rs` and `latest.txt`.
- **Linux packaging:** if Flatpak came first, the AppImage step would be
  swapped; the `.tar.gz` stays useful either way.
- **macOS:** no release until the Apple developer decision; then a
  signed, notarised universal app would be added.
