#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MANIFEST="$ROOT/native/libghostty/manifest.toml"
SOURCE_DIR="${PANEFLOW_GHOSTTY_SOURCE_DIR:-}"
VERIFY_REPRODUCIBLE=0
TARGETS=()

manifest_string() {
  local key="$1"
  sed -n "s/^${key} = \"\(.*\)\"$/\1/p" "$MANIFEST"
}

while (($#)); do
  case "$1" in
    --target)
      [[ $# -ge 2 ]] || { echo "--target requires a Rust target triple" >&2; exit 2; }
      TARGETS+=("$2")
      shift 2
      ;;
    --verify-reproducible)
      VERIFY_REPRODUCIBLE=1
      shift
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

SOURCE_SHA="$(manifest_string source_sha)"
ZIG_VERSION="$(manifest_string zig_version)"
HEADER_PATH="$(manifest_string header_path)"
HEADER_SHA256="$(manifest_string header_sha256)"
BINDINGS_PATH="$(manifest_string bindings_path)"
BINDINGS_SHA256="$(manifest_string bindings_sha256)"
BUILD_MODE="$(manifest_string build_mode)"
ARCHIVE_NORMALIZATION="$(manifest_string archive_normalization)"
BUILD_INFO_SYMBOL="$(manifest_string build_info_symbol)"

[[ -n "$SOURCE_DIR" ]] || {
  echo "PANEFLOW_GHOSTTY_SOURCE_DIR must point to Ghostty $SOURCE_SHA" >&2
  exit 1
}
git -C "$SOURCE_DIR" rev-parse --is-inside-work-tree >/dev/null 2>&1 || {
  echo "$SOURCE_DIR is not a Ghostty Git checkout" >&2
  exit 1
}
ACTUAL_SHA="$(git -C "$SOURCE_DIR" rev-parse HEAD)"
[[ "$ACTUAL_SHA" == "$SOURCE_SHA" ]] || {
  echo "Ghostty source mismatch: expected $SOURCE_SHA, got $ACTUAL_SHA" >&2
  exit 1
}
SOURCE_STATUS="$(git -C "$SOURCE_DIR" status --porcelain --untracked-files=all)"
[[ -z "$SOURCE_STATUS" ]] || {
  echo "Ghostty source must be a clean checkout of $SOURCE_SHA" >&2
  printf '%s\n' "$SOURCE_STATUS" >&2
  exit 1
}
command -v zig >/dev/null || {
  echo "libghostty requires Zig $ZIG_VERSION; install or select the pinned toolchain" >&2
  exit 1
}
ACTUAL_ZIG="$(zig version)"
[[ "$ACTUAL_ZIG" == "$ZIG_VERSION" ]] || {
  echo "libghostty requires Zig $ZIG_VERSION, found $ACTUAL_ZIG" >&2
  exit 1
}
command -v sha256sum >/dev/null || { echo "sha256sum is required" >&2; exit 1; }
command -v nm >/dev/null || { echo "nm is required to verify exported symbols" >&2; exit 1; }
command -v ar >/dev/null || { echo "ar is required to normalize release archives" >&2; exit 1; }
command -v eu-strip >/dev/null || { echo "eu-strip from elfutils is required to normalize release archives" >&2; exit 1; }

ACTUAL_HEADER_SHA256="$(sha256sum "$SOURCE_DIR/$HEADER_PATH" | awk '{print $1}')"
[[ "$ACTUAL_HEADER_SHA256" == "$HEADER_SHA256" ]] || {
  echo "Ghostty header checksum mismatch: expected $HEADER_SHA256, got $ACTUAL_HEADER_SHA256" >&2
  exit 1
}
ACTUAL_BINDINGS_SHA256="$(sha256sum "$ROOT/$BINDINGS_PATH" | awk '{print $1}')"
[[ "$ACTUAL_BINDINGS_SHA256" == "$BINDINGS_SHA256" ]] || {
  echo "Paneflow bindings checksum mismatch: expected $BINDINGS_SHA256, got $ACTUAL_BINDINGS_SHA256" >&2
  exit 1
}

if ((${#TARGETS[@]} == 0)); then
  TARGETS=("x86_64-unknown-linux-gnu" "aarch64-unknown-linux-gnu")
fi

zig_target() {
  case "$1" in
    x86_64-unknown-linux-gnu) echo "x86_64-linux-gnu" ;;
    aarch64-unknown-linux-gnu) echo "aarch64-linux-gnu" ;;
    *) echo "unsupported Linux target: $1" >&2; return 1 ;;
  esac
}

normalize_archive() {
  local archive="$1"
  local normalize_dir="$archive.normalize"
  local normalized="$archive.normalized"
  local members=()
  local basenames=()
  local duplicates

  mapfile -t members < <(ar t "$archive")
  for member in "${members[@]}"; do
    basenames+=("${member##*/}")
  done
  duplicates="$(printf '%s\n' "${basenames[@]}" | sort | uniq -d)"
  [[ -z "$duplicates" ]] || {
    echo "archive normalization found duplicate member names: $duplicates" >&2
    return 1
  }
  # Zig may append members in parallel completion order. Rebuild from a
  # canonical order so identical object files always produce identical bytes.
  mapfile -t basenames < <(printf '%s\n' "${basenames[@]}" | LC_ALL=C sort)

  rm -rf "$normalize_dir" "$normalized"
  mkdir -p "$normalize_dir"
  (
    cd "$normalize_dir" || exit 1
    ar x "$archive" 2>/dev/null || exit 1
    for basename in "${basenames[@]}"; do
      [[ -f "$basename" ]] || exit 1
      eu-strip --strip-debug "$basename" || exit 1
    done
    ar crsD "$normalized" "${basenames[@]}" || exit 1
  ) || {
    local status=$?
    rm -rf "$normalize_dir" "$normalized"
    return "$status"
  }
  mv "$normalized" "$archive"
  rm -rf "$normalize_dir"
}

build_one() {
  local rust_target="$1"
  local output="$2"
  local cache="$3"
  local target
  target="$(zig_target "$rust_target")"
  rm -rf "$output" "$cache"
  mkdir -p "$output" "$cache"
  (
    cd "$SOURCE_DIR"
    ZIG_GLOBAL_CACHE_DIR="$cache/global" ZIG_LOCAL_CACHE_DIR="$cache/local" zig build \
      -Demit-lib-vt=true \
      -Dtarget="$target" \
      -Doptimize="$BUILD_MODE" \
      --prefix "$output"
  )
  local archive="$output/lib/libghostty-vt.a"
  [[ -f "$archive" ]] || { echo "missing static archive: $archive" >&2; return 1; }
  [[ -f "$output/$HEADER_PATH" ]] || { echo "missing installed header: $output/$HEADER_PATH" >&2; return 1; }
  case "$ARCHIVE_NORMALIZATION" in
    elfutils-strip-debug+ar-D) normalize_archive "$archive" ;;
    *) echo "unsupported archive normalization: $ARCHIVE_NORMALIZATION" >&2; return 1 ;;
  esac
  nm -g --defined-only "$archive" | grep -E "[[:space:]]${BUILD_INFO_SYMBOL}$" >/dev/null || {
    echo "archive does not export $BUILD_INFO_SYMBOL: $archive" >&2
    return 1
  }
  local archive_sha
  archive_sha="$(sha256sum "$archive" | awk '{print $1}')"
  cp "$ROOT/$BINDINGS_PATH" "$output/bindings.rs"
  {
    echo "source_sha=$SOURCE_SHA"
    echo "zig_version=$ZIG_VERSION"
    echo "header_sha256=$HEADER_SHA256"
    echo "bindings_sha256=$BINDINGS_SHA256"
    echo "rust_target=$rust_target"
    echo "zig_target=$target"
    echo "optimize=$BUILD_MODE"
    echo "archive_normalization=$ARCHIVE_NORMALIZATION"
    echo "archive_sha256=$archive_sha"
    echo "build_info_symbol=$BUILD_INFO_SYMBOL"
  } > "$output/build-info.txt"
}

for rust_target in "${TARGETS[@]}"; do
  output="$ROOT/target/libghostty/$rust_target"
  cache="$ROOT/target/libghostty-cache/$rust_target"
  build_one "$rust_target" "$output" "$cache"

  if ((VERIFY_REPRODUCIBLE)); then
    second_output="$(mktemp -d)"
    second_cache="$(mktemp -d)"
    trap 'rm -rf "$second_output" "$second_cache"' EXIT
    build_one "$rust_target" "$second_output" "$second_cache"
    cmp "$output/lib/libghostty-vt.a" "$second_output/lib/libghostty-vt.a"
    cmp "$output/$HEADER_PATH" "$second_output/$HEADER_PATH"
    cmp "$output/bindings.rs" "$second_output/bindings.rs"
    cmp "$output/build-info.txt" "$second_output/build-info.txt"
    rm -rf "$second_output" "$second_cache"
    trap - EXIT
  fi

  echo "prepared $rust_target at $output"
done
