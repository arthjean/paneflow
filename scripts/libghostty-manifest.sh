#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
MANIFEST="$ROOT/native/libghostty/manifest.toml"

if [ "$#" -ne 1 ] || [ -z "$1" ]; then
  echo "usage: libghostty-manifest.sh <key>" >&2
  echo "keys: any top-level string key of native/libghostty/manifest.toml, or macos_rust_toolchain" >&2
  exit 2
fi
KEY="$1"

if [ ! -f "$MANIFEST" ]; then
  echo "::error::libghostty manifest not found at $MANIFEST" >&2
  exit 1
fi

manifest_value() {
  local wanted="$1" line name value
  while IFS= read -r line || [ -n "$line" ]; do
    line="${line%$'\r'}"
    if [[ "$line" =~ ^[[:space:]]*\[ ]]; then
      break
    fi
    if [[ "$line" =~ ^([A-Za-z0-9_]+)[[:space:]]*=[[:space:]]*(.*)$ ]]; then
      name="${BASH_REMATCH[1]}"
      value="${BASH_REMATCH[2]}"
      [ "$name" = "$wanted" ] || continue
      if [[ "$value" =~ ^\"([^\"]*)\"[[:space:]]*$ ]]; then
        value="${BASH_REMATCH[1]}"
      fi
      if [ -z "$value" ]; then
        echo "::error::libghostty manifest key '$wanted' is empty in $MANIFEST" >&2
        return 1
      fi
      printf '%s\n' "$value"
      return 0
    fi
  done < "$MANIFEST"
  echo "::error::libghostty manifest has no top-level key '$wanted' ($MANIFEST)" >&2
  return 1
}

case "$KEY" in
  macos_rust_toolchain)
    llvm="$(manifest_value macos_llvm_version)"
    if ! [[ "$llvm" =~ -rust-([0-9][0-9A-Za-z.-]*)-stable$ ]]; then
      echo "::error::macos_llvm_version '$llvm' does not name a Rust toolchain (<llvm>-rust-<version>-stable)" >&2
      exit 1
    fi
    printf '%s\n' "${BASH_REMATCH[1]}"
    ;;
  *)
    manifest_value "$KEY"
    ;;
esac
