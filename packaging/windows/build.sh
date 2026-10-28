#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Builds the Windows installer and a .zip from a built daw.exe. Runs on
# Linux with makensis (the release workflow does this).
#
#   packaging/windows/build.sh <version> <path to daw.exe> <output folder>
set -euo pipefail
version=$1 exe=$2 out=$3
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
mkdir -p "$out"
out=$(cd "$out" && pwd)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

name="daw-$version-windows-x64"
mkdir "$stage/$name"
cp "$exe" "$stage/$name/daw.exe"
# Windows line endings, so Notepad shows them properly.
sed 's/$/\r/' "$root/LICENSE" > "$stage/$name/LICENSE.txt"
sed 's/$/\r/' "$root/packaging/README.txt" > "$stage/$name/README.txt"

makensis -V2 -DVERSION="$version" -DNUMVERSION="${version%%-*}" -DSRC="$stage/$name" \
  -DOUT="$out/$name-setup.exe" "$here/installer.nsi"
(cd "$stage" && zip -q -r -X "$out/$name.zip" "$name")
