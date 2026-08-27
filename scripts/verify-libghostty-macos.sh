#!/usr/bin/env bash
set -euo pipefail

# Mach-O analog of scripts/verify-libghostty-package.sh. It proves that a
# packaged macOS binary is Apple Silicon, links the pinned libghostty-vt
# archive statically, and carries the reviewed native notice. The Linux script
# is deliberately untouched: the two share a contract, not an implementation.

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

# macOS ships shasum, Linux ships sha256sum, and this script has to run on both
# a release runner and a maintainer laptop. Resolve one of them up front rather
# than degrading to an unchecked notice.
if command -v sha256sum >/dev/null; then
  sha256_of() { sha256sum "$1" | awk '{print $1}'; }
elif command -v shasum >/dev/null; then
  sha256_of() { shasum -a 256 "$1" | awk '{print $1}'; }
else
  echo "neither sha256sum nor shasum is available to hash the native notice" >&2
  exit 1
fi

# otool is the Apple spelling, llvm-objdump the portable one. Never skip the
# load-command inspection: a missing toolchain is a failure, not a pass.
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

# `file` word order differs between Apple file(1) ("Mach-O 64-bit executable
# arm64") and GNU file(1) ("Mach-O 64-bit arm64 executable"), so match the
# three tokens independently instead of one host-specific phrase.
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
# Guard the shape too: an empty SOURCE_SHA would turn the linkage proof below
# into `grep -aFq ""`, which matches every byte stream and passes a binary with
# no engine linked at all.
[[ "$SOURCE_SHA" =~ ^[0-9a-f]{40}$ ]] || {
  echo "manifest contains an invalid pinned Ghostty source SHA" >&2
  exit 1
}
ACTUAL_NOTICE_SHA="$(sha256_of "$NOTICE")"
[[ "$ACTUAL_NOTICE_SHA" == "$NOTICE_SHA" ]] || {
  echo "packaged native notice does not match the reviewed manifest hash" >&2
  exit 1
}

# `paneflow-terminal-ghostty` embeds the whole pinned manifest through
# include_str! under cfg(ghostty_native), so the pinned source SHA is present
# only when the engine is actually linked. A build without it fails here.
grep -aFq "$SOURCE_SHA" "$BINARY" || {
  echo "packaged binary does not contain the pinned Ghostty build identity" >&2
  exit 1
}

echo "packaged libghostty linkage and native notices verified"
