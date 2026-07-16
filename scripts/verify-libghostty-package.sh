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
command -v sha256sum >/dev/null || { echo "sha256sum is required" >&2; exit 1; }
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
NOTICE_PATH="$(sed -n 's/^notice_path = "\(.*\)"$/\1/p' "$ROOT/native/libghostty/manifest.toml")"
NOTICE_SHA="$(sed -n 's/^notice_sha256 = "\(.*\)"$/\1/p' "$ROOT/native/libghostty/manifest.toml")"
[[ -f "$ROOT/$NOTICE_PATH" ]] || {
  echo "manifest references a missing native notice: $NOTICE_PATH" >&2
  exit 1
}
[[ "$NOTICE_SHA" =~ ^[0-9a-f]{64}$ ]] || {
  echo "manifest contains an invalid native notice hash" >&2
  exit 1
}
read -r ACTUAL_NOTICE_SHA _ < <(sha256sum "$NOTICE")
[[ "$ACTUAL_NOTICE_SHA" == "$NOTICE_SHA" ]] || {
  echo "packaged native notice does not match the reviewed manifest hash" >&2
  exit 1
}
grep -aFq "$SOURCE_SHA" "$BINARY" || {
  echo "packaged binary does not contain the pinned Ghostty build identity" >&2
  exit 1
}
for component in \
  'Artifact member inventory' \
  'Ghostty / libghostty-vt' \
  'Zig compiler runtime' \
  uucode \
  'Unicode Character Database' \
  'Bjoern Hoehrmann UTF-8 DFA' \
  'X.Org rgb data' \
  'foot kitty keymap' \
  simdutf \
  Highway \
  'LLVM libc++ headers'; do
  grep -Fq "$component" "$NOTICE" || {
    echo "native notice is missing $component" >&2
    exit 1
  }
done
echo "packaged libghostty linkage and native notices verified"
