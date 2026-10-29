#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Builds the Linux AppImage and .tar.gz from a built `daw` binary
# (roadmap 1.8; AppImage plus .tar.gz is the 0.7 proposal in
# docs/research/PLATFORM_DISTRIBUTION.md).
#
#   packaging/linux/build.sh <version> <path to daw> <output folder>
#
# APPIMAGETOOL and APPIMAGE_RUNTIME name the packaging tool and the
# runtime it puts at the front of the AppImage; the release workflow
# downloads pinned, checksummed copies. Without APPIMAGETOOL only the
# .tar.gz is made.
#
# Nothing is bundled beyond the app itself: it needs the system's ALSA
# library and graphics drivers, as other desktop apps do. Build on the
# oldest supported distribution so the C library it links against is
# old enough for newer ones.
set -euo pipefail
version=$1 bin=$2 out=$3
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
mkdir -p "$out"
out=$(cd "$out" && pwd)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

name="daw-$version-linux-x86_64"

# The .tar.gz: the binary with its licence, readme, desktop entry and
# icon, to unpack anywhere.
mkdir "$stage/$name"
install -m 755 "$bin" "$stage/$name/daw"
cp "$root/LICENSE" "$root/packaging/README.txt" "$here/daw.desktop" "$stage/$name/"
cp "$root/packaging/icon.png" "$stage/$name/daw.png"
tar -C "$stage" --owner=0 --group=0 -czf "$out/$name.tar.gz" "$name"

if [ -z "${APPIMAGETOOL:-}" ]; then
  echo "APPIMAGETOOL not set: made the .tar.gz only"
  exit 0
fi

# The AppImage: one file that runs in place.
app="$stage/AppDir"
mkdir -p "$app/usr/bin" "$app/usr/share/applications" \
  "$app/usr/share/icons/hicolor/256x256/apps" "$app/usr/share/doc/daw"
install -m 755 "$bin" "$app/usr/bin/daw"
cp "$here/daw.desktop" "$app/usr/share/applications/daw.desktop"
cp "$root/packaging/icon.png" "$app/usr/share/icons/hicolor/256x256/apps/daw.png"
cp "$root/LICENSE" "$root/packaging/README.txt" "$app/usr/share/doc/daw/"
ln -s usr/share/applications/daw.desktop "$app/daw.desktop"
ln -s usr/share/icons/hicolor/256x256/apps/daw.png "$app/daw.png"
cat > "$app/AppRun" <<'RUN'
#!/bin/sh
here="$(dirname "$(readlink -f "$0")")"
exec "$here/usr/bin/daw" "$@"
RUN
chmod 755 "$app/AppRun"

runtime=()
if [ -n "${APPIMAGE_RUNTIME:-}" ]; then
  runtime=(--runtime-file "$APPIMAGE_RUNTIME")
fi
# The tool is itself an AppImage; extract-and-run avoids needing FUSE on
# the build machine.
ARCH=x86_64 "$APPIMAGETOOL" --appimage-extract-and-run --no-appstream \
  "${runtime[@]}" "$app" "$out/$name.AppImage"
