#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: stage-macos-assets.sh <version> <arch>" >&2
  echo "  copies dist/paneflow-<version>-<arch>-apple-darwin.dmg into release-assets/ with its .sha256" >&2
  exit 2
fi
version="${1#v}"
ARCH="$2"

mkdir -p release-assets
dmg="paneflow-${version}-${ARCH}-apple-darwin.dmg"
cp "dist/${dmg}" "release-assets/${dmg}"
( cd release-assets && shasum -a 256 "${dmg}" > "${dmg}.sha256" )
ls -la release-assets/
