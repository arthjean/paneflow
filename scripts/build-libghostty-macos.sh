#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MANIFEST="$ROOT/native/libghostty/manifest.toml"
SOURCE_DIR="${PANEFLOW_GHOSTTY_SOURCE_DIR:-}"
VERIFY_REPRODUCIBLE=0
ALLOW_HASH_DRIFT=0
TARGETS=()

manifest_string() {
  local key="$1"
  sed -n "s/^${key} = \"\(.*\)\"$/\1/p" "$MANIFEST"
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
  local platform="$1"
  awk -v platform="$platform" '
    /^\[targets\."[^"]+"\]$/ {
      target = $0
      sub(/^\[targets\."/, "", target)
      sub(/"\]$/, "", target)
      in_target = 1
      next
    }
    in_target && /^platform = "/ {
      value = $0
      sub(/^platform = "/, "", value)
      sub(/"$/, "", value)
      if (value == platform) print target
      in_target = 0
    }
  ' "$MANIFEST"
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
    --allow-hash-drift)
      ALLOW_HASH_DRIFT=1
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
LLVM_VERSION="$(manifest_string macos_llvm_version)"
BUILD_SEED="$(manifest_string macos_build_seed)"
BUILD_JOBS="$(manifest_string macos_build_jobs)"
CANONICAL_ROOT="$(manifest_string macos_canonical_source_path)"
CANONICAL_ZIG_ROOT="$(manifest_string macos_canonical_zig_path)"

for key in macos_llvm_version macos_build_seed macos_build_jobs macos_canonical_source_path macos_canonical_zig_path; do
  [[ -n "$(manifest_string "$key")" ]] || { echo "manifest is missing $key" >&2; exit 1; }
done

CANONICAL_CACHE="$CANONICAL_ROOT/.paneflow-zig-cache"
CANONICAL_PREFIX="$CANONICAL_ROOT/.paneflow-zig-output"
case "$CANONICAL_ROOT" in
  /*/paneflow-libghostty-*) ;;
  *)
    echo "macos_canonical_source_path must be an absolute paneflow-libghostty-* path" >&2
    exit 1
    ;;
esac
case "$CANONICAL_ZIG_ROOT" in
  /*/paneflow-libghostty-zig-*) ;;
  *)
    echo "macos_canonical_zig_path must be an absolute paneflow-libghostty-zig-* path" >&2
    exit 1
    ;;
esac

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
ZIG_ENV="$(zig env)"
ZIG_EXE="$(printf '%s\n' "$ZIG_ENV" | sed -n 's/^ *\.zig_exe = "\(.*\)",$/\1/p')"
ZIG_LIB_DIR="$(printf '%s\n' "$ZIG_ENV" | sed -n 's/^ *\.lib_dir = "\(.*\)",$/\1/p')"
[[ -x "$ZIG_EXE" && -d "$ZIG_LIB_DIR" ]] || {
  echo "could not resolve the pinned Zig executable and lib directory from zig env" >&2
  exit 1
}
command -v sha256sum >/dev/null || { echo "sha256sum is required" >&2; exit 1; }
command -v file >/dev/null || { echo "file is required to verify Mach-O members" >&2; exit 1; }
command -v tar >/dev/null || { echo "tar is required to export the canonical source tree" >&2; exit 1; }
command -v taskset >/dev/null || { echo "taskset from util-linux is required to pin the Zig compiler to one CPU" >&2; exit 1; }
BUILD_CPU="$(taskset -cp $$ | sed 's/.*: //' | cut -d, -f1 | cut -d- -f1)"

LLVM_BIN="${PANEFLOW_LLVM_BIN:-}"
if [[ -z "$LLVM_BIN" ]] && command -v rustc >/dev/null; then
  candidate="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin"
  [[ -x "$candidate/llvm-strip" ]] && LLVM_BIN="$candidate"
fi

llvm_tool() {
  local name="$1"
  local resolved=""
  if [[ -n "$LLVM_BIN" && -x "$LLVM_BIN/$name" ]]; then
    resolved="$LLVM_BIN/$name"
  else
    resolved="$(command -v "$name" || true)"
  fi
  [[ -n "$resolved" ]] || {
    echo "libghostty requires $name from LLVM $LLVM_VERSION" >&2
    echo "install the rustup llvm-tools component or set PANEFLOW_LLVM_BIN" >&2
    return 1
  }
  printf '%s' "$resolved"
}

LLVM_STRIP="$(llvm_tool llvm-strip)"
LLVM_AR="$(llvm_tool llvm-ar)"
LLVM_NM="$(llvm_tool llvm-nm)"
LLVM_OBJDUMP="$(llvm_tool llvm-objdump)"

for tool in "$LLVM_STRIP" "$LLVM_AR" "$LLVM_NM" "$LLVM_OBJDUMP"; do
  "$tool" --version 2>&1 | grep -Fq "LLVM version $LLVM_VERSION" || {
    echo "libghostty requires LLVM $LLVM_VERSION for $tool" >&2
    "$tool" --version >&2 || true
    exit 1
  }
done

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
  mapfile -t TARGETS < <(manifest_targets macos)
  ((${#TARGETS[@]} > 0)) || { echo "manifest declares no macOS libghostty targets" >&2; exit 1; }
fi

remove_canonical_source() {
  case "$CANONICAL_ROOT" in
    /*/paneflow-libghostty-*) rm -rf "$CANONICAL_ROOT" ;;
    *) echo "refusing to remove unexpected canonical source path $CANONICAL_ROOT" >&2 ;;
  esac
}

remove_canonical_zig() {
  case "$CANONICAL_ZIG_ROOT" in
    /*/paneflow-libghostty-zig-*) rm -rf "$CANONICAL_ZIG_ROOT" ;;
    *) echo "refusing to remove unexpected canonical zig path $CANONICAL_ZIG_ROOT" >&2 ;;
  esac
}

prepare_canonical_zig() {
  [[ ! -e "$CANONICAL_ZIG_ROOT" ]] || {
    echo "canonical libghostty zig path is already in use: $CANONICAL_ZIG_ROOT" >&2
    return 1
  }
  mkdir -p "$CANONICAL_ZIG_ROOT"
  cp "$ZIG_EXE" "$CANONICAL_ZIG_ROOT/zig"
  cp -R "$ZIG_LIB_DIR/." "$CANONICAL_ZIG_ROOT/lib/"
  local staged_version
  staged_version="$("$CANONICAL_ZIG_ROOT/zig" version)"
  [[ "$staged_version" == "$ZIG_VERSION" ]] || {
    echo "staged canonical Zig reports $staged_version, expected $ZIG_VERSION" >&2
    return 1
  }
}

prepare_canonical_source() {
  [[ ! -e "$CANONICAL_ROOT" ]] || {
    echo "canonical libghostty source path is already in use: $CANONICAL_ROOT" >&2
    return 1
  }
  mkdir -p "$CANONICAL_ROOT"
  git -C "$SOURCE_DIR" archive --format=tar "$SOURCE_SHA" | tar -x -C "$CANONICAL_ROOT"
  local exported_sha
  exported_sha="$(sha256sum "$CANONICAL_ROOT/$HEADER_PATH" | awk '{print $1}')"
  [[ "$exported_sha" == "$HEADER_SHA256" ]] || {
    echo "canonical Ghostty export has an unexpected header checksum" >&2
    return 1
  }
}

normalize_archive() {
  local archive="$1"
  local normalize_dir="$archive.normalize"
  local normalized="$archive.normalized"
  local members=()
  local basenames=()
  local duplicates

  mapfile -t members < <("$LLVM_AR" t "$archive")
  for member in "${members[@]}"; do
    basenames+=("${member##*/}")
  done
  duplicates="$(printf '%s\n' "${basenames[@]}" | sort | uniq -d)"
  [[ -z "$duplicates" ]] || {
    echo "archive normalization found duplicate member names: $duplicates" >&2
    return 1
  }
  mapfile -t basenames < <(printf '%s\n' "${basenames[@]}" | LC_ALL=C sort)

  rm -rf "$normalize_dir" "$normalized"
  mkdir -p "$normalize_dir"
  (
    cd "$normalize_dir" || exit 1
    "$LLVM_AR" x "$archive" 2>/dev/null || exit 1
    for basename in "${basenames[@]}"; do
      [[ -f "$basename" ]] || exit 1
      file -b "$basename" | grep -q "Mach-O 64-bit arm64 object" || {
        echo "archive member is not a Mach-O arm64 object: $basename" >&2
        exit 1
      }
      "$LLVM_STRIP" -S "$basename" || exit 1
    done
    "$LLVM_AR" crsD --format=darwin "$normalized" "${basenames[@]}" || exit 1
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
  local target
  local archive_path
  local archive_normalization
  local build_info_symbol
  target="$(target_manifest_string "$rust_target" zig_target)"
  archive_path="$(target_manifest_string "$rust_target" archive_path)"
  archive_normalization="$(target_manifest_string "$rust_target" archive_normalization)"
  build_info_symbol="$(target_manifest_string "$rust_target" build_info_symbol)"

  prepare_canonical_source
  (
    cd "$CANONICAL_ROOT"
    ZIG_GLOBAL_CACHE_DIR="$CANONICAL_CACHE/global" ZIG_LOCAL_CACHE_DIR="$CANONICAL_CACHE/local" \
      taskset -c "$BUILD_CPU" "$CANONICAL_ZIG_ROOT/zig" build \
      --zig-lib-dir "$CANONICAL_ZIG_ROOT/lib" \
      -Demit-lib-vt=true \
      -Dtarget="$target" \
      -Doptimize="$BUILD_MODE" \
      --seed "$BUILD_SEED" \
      -j"$BUILD_JOBS" \
      --prefix "$CANONICAL_PREFIX"
  )
  rm -rf "$output"
  mkdir -p "$output"
  cp -R "$CANONICAL_PREFIX/." "$output/"
  remove_canonical_source

  local archive="$output/$archive_path"
  [[ -f "$archive" ]] || { echo "missing static archive: $archive" >&2; return 1; }
  [[ -f "$output/$HEADER_PATH" ]] || { echo "missing installed header: $output/$HEADER_PATH" >&2; return 1; }
  case "$archive_normalization" in
    fixed-zig-source-cache-prefix+zig-build-seed0-j1-cpu1+llvm-strip-debug+llvm-ar-D-darwin) normalize_archive "$archive" ;;
    *) echo "unsupported archive normalization: $archive_normalization" >&2; return 1 ;;
  esac
  "$LLVM_NM" -g --defined-only "$archive" | grep -E "[[:space:]]_?${build_info_symbol}$" >/dev/null || {
    echo "archive does not export $build_info_symbol: $archive" >&2
    return 1
  }
  local archive_sha
  archive_sha="$(sha256sum "$archive" | awk '{print $1}')"
  local expected_archive_sha
  expected_archive_sha="$(target_manifest_string "$rust_target" archive_sha256)"
  if [[ "$archive_sha" != "$expected_archive_sha" ]]; then
    if ((ALLOW_HASH_DRIFT)); then
      echo "warning: canonical macOS archive hash differs from manifest; expected $expected_archive_sha, got $archive_sha" >&2
    else
      echo "canonical macOS archive hash differs from manifest; expected $expected_archive_sha, got $archive_sha" >&2
      return 1
    fi
  fi
  cp "$ROOT/$BINDINGS_PATH" "$output/bindings.rs"
  {
    echo "source_sha=$SOURCE_SHA"
    echo "zig_version=$ZIG_VERSION"
    echo "header_sha256=$HEADER_SHA256"
    echo "bindings_sha256=$BINDINGS_SHA256"
    echo "rust_target=$rust_target"
    echo "zig_target=$target"
    echo "optimize=$BUILD_MODE"
    echo "archive_normalization=$archive_normalization"
    echo "archive_sha256=$archive_sha"
    echo "build_info_symbol=$build_info_symbol"
  } > "$output/build-info.txt"
}

report_archive_divergence() {
  local first="$1"
  local second="$2"
  local offset
  offset="$(LC_ALL=C cmp "$first" "$second" 2>&1 | sed -n 's/.*differ: [^0-9]*\([0-9][0-9]*\).*/\1/p')"
  echo "macOS archive is not reproducible; first differing byte offset: ${offset:-unknown}" >&2

  local dir
  dir="$(mktemp -d)"
  mkdir -p "$dir/a" "$dir/b"
  ( cd "$dir/a" && "$LLVM_AR" x "$first" ) || true
  ( cd "$dir/b" && "$LLVM_AR" x "$second" ) || true
  local member
  for member in "$dir"/a/*; do
    [[ -f "$member" ]] || continue
    local name="${member##*/}"
    if ! cmp -s "$dir/a/$name" "$dir/b/$name"; then
      echo "divergent member: $name" >&2
      echo "--- first build headers ---" >&2
      "$LLVM_OBJDUMP" --macho --all-headers "$dir/a/$name" >&2 || true
      echo "--- second build headers ---" >&2
      "$LLVM_OBJDUMP" --macho --all-headers "$dir/b/$name" >&2 || true
    fi
  done
  rm -rf "$dir"
}

FIRST_PASS=""

cleanup() {
  remove_canonical_source
  remove_canonical_zig
  [[ -z "$FIRST_PASS" ]] || rm -rf "$FIRST_PASS"
}

trap cleanup EXIT

prepare_canonical_zig

for rust_target in "${TARGETS[@]}"; do
  output="$ROOT/target/libghostty/$rust_target"
  build_one "$rust_target" "$output"

  if ((VERIFY_REPRODUCIBLE)); then
    archive_path="$(target_manifest_string "$rust_target" archive_path)"
    FIRST_PASS="$(mktemp -d)"
    cp -R "$output/." "$FIRST_PASS/"
    build_one "$rust_target" "$output"
    if ! cmp "$FIRST_PASS/$archive_path" "$output/$archive_path"; then
      report_archive_divergence "$FIRST_PASS/$archive_path" "$output/$archive_path"
      exit 1
    fi
    cmp "$FIRST_PASS/$HEADER_PATH" "$output/$HEADER_PATH"
    cmp "$FIRST_PASS/bindings.rs" "$output/bindings.rs"
    cmp "$FIRST_PASS/build-info.txt" "$output/build-info.txt"
    rm -rf "$FIRST_PASS"
    FIRST_PASS=""
  fi

  echo "prepared $rust_target at $output"
done
