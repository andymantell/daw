#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Builds the macOS .dmg holding a universal DAW.app (Apple Silicon and
# Intel in one bundle, the 0.7 proposal in
# docs/research/PLATFORM_DISTRIBUTION.md) from the two built `daw`
# binaries. Runs on macOS: it needs lipo, sips, iconutil, codesign and
# hdiutil, which come with the system and its command line tools.
#
#   packaging/macos/build.sh <version> <arm64 daw> <x86_64 daw> <output folder>
#
# MACOS_MIN names the oldest macOS the bundle says it runs on (default
# 13.0); build the binaries with MACOSX_DEPLOYMENT_TARGET set to the same.
#
# The app is not signed by a developer (owner's choice, 2026-11-05): it
# gets only an ad-hoc signature, which Apple Silicon needs to run it at
# all. macOS warns on first launch; docs/RELEASING.md has the steps.
set -euo pipefail
version=$1 arm=$2 intel=$3 out=$4
min=${MACOS_MIN:-13.0}
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
mkdir -p "$out"
out=$(cd "$out" && pwd)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

name="daw-$version-macos-universal"
app="$stage/dmg/DAW.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

# One binary holding both architectures.
lipo -create -output "$app/Contents/MacOS/daw" "$arm" "$intel"
chmod 755 "$app/Contents/MacOS/daw"
lipo -verify_arch "$app/Contents/MacOS/daw" arm64 x86_64

# The bundle version must be numbers only, so a pre-release suffix such
# as -beta.1 stays in CFBundleVersion alone.
sed -e "s/@VERSION@/$version/" -e "s/@NUMVERSION@/${version%%-*}/" \
  -e "s/@MINOS@/$min/" "$here/Info.plist" > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"

# The icon, from the shared 256-pixel picture.
set_dir="$stage/DAW.iconset"
mkdir "$set_dir"
for s in 16 32 128 256; do
  sips -z "$s" "$s" "$root/packaging/icon.png" --out "$set_dir/icon_${s}x${s}.png" >/dev/null
done
cp "$set_dir/icon_32x32.png" "$set_dir/icon_16x16@2x.png"
sips -z 64 64 "$root/packaging/icon.png" --out "$set_dir/icon_32x32@2x.png" >/dev/null
cp "$set_dir/icon_256x256.png" "$set_dir/icon_128x128@2x.png"
iconutil -c icns -o "$app/Contents/Resources/DAW.icns" "$set_dir"

cp "$root/LICENSE" "$root/packaging/README.txt" "$app/Contents/Resources/"

# Ad-hoc signature over the whole bundle, so the Info.plist and
# resources are sealed with the binary.
codesign --force --sign - --timestamp=none "$app"
codesign --verify --strict --verbose=2 "$app"

# The disk image: the app, a link to Applications to drag it onto, and
# the readme and licence.
ln -s /Applications "$stage/dmg/Applications"
cp "$root/LICENSE" "$stage/dmg/LICENSE.txt"
cp "$root/packaging/README.txt" "$stage/dmg/README.txt"
hdiutil create -quiet -volname "DAW $version" -srcfolder "$stage/dmg" \
  -fs HFS+ -format UDZO -ov "$out/$name.dmg"
