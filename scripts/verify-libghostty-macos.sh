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
command -v file >/dev/null || { echo "file is required to identify the Mach-O binary" >&2; exit 1; }

if command -v sha256sum >/dev/null; then
  sha256_of() { sha256sum "$1" | awk '{print $1}'; }
elif command -v shasum >/dev/null; then
  sha256_of() { shasum -a 256 "$1" | awk '{print $1}'; }
else
  echo "neither sha256sum nor shasum is available to hash the native notice" >&2
  exit 1
fi

if command -v otool >/dev/null; then
  DYLIBS="$(otool -L "$BINARY")" || {
    echo "packaged PaneFlow binary has unreadable Mach-O load commands" >&2
    exit 1
  }
elif command -v llvm-objdump >/dev/null; then
  DYLIBS="$(llvm-objdump --macho --dylibs-used "$BINARY")" || {
    echo "packaged PaneFlow binary has unreadable Mach-O load commands" >&2
    exit 1
  }
else
  echo "otool is required, and no llvm-objdump fallback is installed" >&2
  exit 1
fi

FILE_DESC="$(file -b "$BINARY")"
case "$FILE_DESC" in
  *universal*)
    echo "packaged PaneFlow binary is a universal binary, expected a single arm64 slice" >&2
    exit 1
    ;;
esac
grep -Fq 'Mach-O 64-bit' <<<"$FILE_DESC" || {
  echo "packaged PaneFlow binary is not a 64-bit Mach-O file: $FILE_DESC" >&2
  exit 1
}
grep -qw executable <<<"$FILE_DESC" || {
  echo "packaged PaneFlow binary is not a Mach-O executable: $FILE_DESC" >&2
  exit 1
}
grep -qw arm64 <<<"$FILE_DESC" || {
  echo "packaged PaneFlow binary is not arm64: $FILE_DESC" >&2
  exit 1
}
if grep -qw x86_64 <<<"$FILE_DESC"; then
  echo "packaged PaneFlow binary reports an x86_64 slice: $FILE_DESC" >&2
  exit 1
fi

if grep -Eq 'libghostty[^[:space:]]*\.dylib' <<<"$DYLIBS"; then
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
[[ "$SOURCE_SHA" =~ ^[0-9a-f]{40}$ ]] || {
  echo "manifest contains an invalid pinned Ghostty source SHA" >&2
  exit 1
}
ACTUAL_NOTICE_SHA="$(sha256_of "$NOTICE")"
[[ "$ACTUAL_NOTICE_SHA" == "$NOTICE_SHA" ]] || {
  echo "packaged native notice does not match the reviewed manifest hash" >&2
  exit 1
}

grep -aFq "$SOURCE_SHA" "$BINARY" || {
  echo "packaged binary does not contain the pinned Ghostty build identity" >&2
  exit 1
}

echo "packaged libghostty linkage and native notices verified"
