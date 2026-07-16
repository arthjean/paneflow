#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
BINARY=""
NOTICE=""
while (($#)); do
  case "$1" in
    --binary) BINARY="${2:-}"; shift 2 ;;
    --notice) NOTICE="${2:-}"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

[[ -f "$BINARY" ]] || { echo "missing packaged PaneFlow binary: $BINARY" >&2; exit 1; }
[[ -f "$NOTICE" ]] || { echo "missing libghostty third-party notice: $NOTICE" >&2; exit 1; }
command -v readelf >/dev/null || { echo "readelf is required" >&2; exit 1; }
case "$(uname -m)" in
  x86_64) EXPECTED_MACHINE='Advanced Micro Devices X86-64' ;;
  aarch64) EXPECTED_MACHINE='AArch64' ;;
  *) echo "unsupported Linux release architecture: $(uname -m)" >&2; exit 1 ;;
esac

ELF_HEADER="$(readelf -h "$BINARY")" || {
  echo "packaged PaneFlow binary is not a readable ELF file" >&2
  exit 1
}
grep -Eq "Machine:[[:space:]]+$EXPECTED_MACHINE$" <<<"$ELF_HEADER" || {
  echo "packaged PaneFlow binary architecture does not match $(uname -m)" >&2
  exit 1
}
ELF_DYNAMIC="$(readelf -d "$BINARY")" || {
  echo "packaged PaneFlow binary has an unreadable ELF dynamic section" >&2
  exit 1
}
if grep -E 'NEEDED.*libghostty[^]]*\.so' <<<"$ELF_DYNAMIC" >/dev/null; then
  echo "packaged binary has a forbidden dynamic libghostty dependency" >&2
  exit 1
fi
SOURCE_SHA="$(sed -n 's/^source_sha = "\(.*\)"$/\1/p' "$ROOT/native/libghostty/manifest.toml")"
grep -aFq "$SOURCE_SHA" "$BINARY" || {
  echo "packaged binary does not contain the pinned Ghostty build identity" >&2
  exit 1
}
for component in 'Ghostty / libghostty-vt' simdutf Highway Zig; do
  grep -Fq "$component" "$NOTICE" || {
    echo "native notice is missing $component" >&2
    exit 1
  }
done
echo "packaged libghostty linkage and native notices verified"
