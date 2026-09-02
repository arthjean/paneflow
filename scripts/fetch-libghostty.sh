#!/usr/bin/env bash
# Places the reviewed libghostty-vt archive for each requested Rust target at
# native/libghostty/prebuilt/<target>/<archive_path>.
#
# The archives are not tracked by git. They live on the GitHub Release that
# native/libghostty/manifest.toml names (`archive_release_repository` and
# `archive_release_tag`), one asset per target, and this script is the only
# step that downloads them: `paneflow-libghostty-sys/build.rs` performs no
# network access and fails with a pointer here when an archive is missing.
#
# Every download is checked against the manifest's `archive_sha256` before it
# is moved into place, so a tampered or mismatched asset never reaches Cargo.
# An archive already in place with the right hash is left alone, which makes
# the script safe to run on every build.
#
# Usage: scripts/fetch-libghostty.sh [--target <rust-triple>]...
#        (no --target: every target the manifest declares)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
MANIFEST="$ROOT/native/libghostty/manifest.toml"
TARGETS=()

manifest_string() {
  sed -n "s/^$1 = \"\(.*\)\"$/\1/p" "$MANIFEST"
}

target_manifest_string() {
  local target="$1"
  local key="$2"
  awk -v target="$target" -v key="$key" '
    $0 == "[targets.\"" target "\"]" { in_target = 1; next }
    in_target && /^\[/ { exit }
    in_target {
      prefix = key " = \""
      if (index($0, prefix) == 1 && substr($0, length($0), 1) == "\"") {
        print substr($0, length(prefix) + 1, length($0) - length(prefix) - 1)
        found = 1
        exit
      }
    }
    END { if (!found) exit 1 }
  ' "$MANIFEST"
}

manifest_targets() {
  sed -n 's/^\[targets\."\([^"]*\)"\]$/\1/p' "$MANIFEST"
}

sha256_of() {
  if command -v sha256sum >/dev/null; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

while (($#)); do
  case "$1" in
    --target)
      [[ $# -ge 2 ]] || { echo "--target requires a Rust target triple" >&2; exit 2; }
      TARGETS+=("$2")
      shift 2
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 2
      ;;
  esac
done
if ((${#TARGETS[@]} == 0)); then
  # No mapfile: macOS ships bash 3.2 at /bin/bash.
  while IFS= read -r target; do
    TARGETS+=("$target")
  done < <(manifest_targets)
fi

REPOSITORY="$(manifest_string archive_release_repository)"
TAG="$(manifest_string archive_release_tag)"
[[ -n "$REPOSITORY" && -n "$TAG" ]] || {
  echo "manifest must name archive_release_repository and archive_release_tag" >&2
  exit 1
}
command -v curl >/dev/null || { echo "curl is required" >&2; exit 1; }

for target in "${TARGETS[@]}"; do
  archive_path="$(target_manifest_string "$target" archive_path)" || {
    echo "manifest declares no target $target" >&2
    exit 1
  }
  expected="$(target_manifest_string "$target" archive_sha256)"
  destination="$ROOT/native/libghostty/prebuilt/$target/$archive_path"
  if [[ -f "$destination" && "$(sha256_of "$destination")" == "$expected" ]]; then
    echo "$target: archive already in place ($expected)"
    continue
  fi

  asset="$target-$(basename "$archive_path")"
  url="https://github.com/$REPOSITORY/releases/download/$TAG/$asset"
  partial="$destination.part"
  mkdir -p "$(dirname "$destination")"
  rm -f "$partial"
  echo "$target: downloading $url"
  curl --fail --silent --show-error --location \
    --retry 3 --retry-delay 5 \
    --output "$partial" "$url"
  actual="$(sha256_of "$partial")"
  if [[ "$actual" != "$expected" ]]; then
    rm -f "$partial"
    echo "$target: downloaded archive hash $actual does not match manifest archive_sha256 $expected" >&2
    exit 1
  fi
  mv -f "$partial" "$destination"
  echo "$target: placed $destination ($expected)"
done
