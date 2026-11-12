#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: compare-libghostty-archive.sh <rust-target> <rebuilt-archive> <published-archive>" >&2
  echo "  fails, naming the target and both SHA-256, unless the two archives are byte-identical" >&2
  exit 2
fi
TARGET="$1"
REBUILT="$2"
PUBLISHED="$3"

for path in "$REBUILT" "$PUBLISHED"; do
  if [ ! -f "$path" ]; then
    echo "::error::$TARGET: missing archive $path"
    exit 1
  fi
done

rebuilt_sha="$(sha256sum "$REBUILT" | awk '{print $1}')"
published_sha="$(sha256sum "$PUBLISHED" | awk '{print $1}')"
if [ "$rebuilt_sha" != "$published_sha" ]; then
  echo "::error::$TARGET: the published archive is not the one the recipe produces: rebuilt $rebuilt_sha, published $published_sha"
  exit 1
fi
echo "$TARGET: the rebuilt archive is the published one ($published_sha)"
