#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Writes latest.txt, the release's list of its version and files with
# their SHA-256 checksums, into the folder holding the release files. The
# release workflow signs it; the app's update check reads the version
# from it once the signature checks out (crates/app/src/update.rs).
#
#   packaging/manifest.sh <version> <folder>
set -euo pipefail
version=$1 dir=$2
cd "$dir"
{
  echo "version $version"
  for f in *; do
    case "$f" in latest.txt | latest.txt.minisig) continue ;; esac
    echo "sha256 $(sha256sum "$f" | cut -d' ' -f1) $f"
  done
} > latest.txt
cat latest.txt
