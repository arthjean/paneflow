#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
HELPER_DIR="${1:-$ROOT/target/embed-build/x86_64-unknown-linux-gnu/release-min}"
BUILD_RS="${PANEFLOW_BUILD_RS:-$ROOT/src-app/build.rs}"

SHIM_HARD=524288
HOOK_HARD=384000
MCP_HARD=524288
SHIM_SOFT=614400
HOOK_SOFT=307200
MARGIN_WARN=4096

total_hard="$(sed -n 's/^const EMBED_SIZE_LIMIT_BYTES: u64 = \([0-9_]*\);.*/\1/p' "$BUILD_RS" | tr -d '_\r')"
if ! [[ "$total_hard" =~ ^[0-9]+$ ]]; then
  echo "::error::could not read EMBED_SIZE_LIMIT_BYTES from $BUILD_RS"
  exit 1
fi

status=0
total=0

measure() {
  local name="$1" hard="$2" soft="$3"
  local path="$HELPER_DIR/$name"
  if [ ! -f "$path" ]; then
    echo "::error::$name is missing from $HELPER_DIR: src-app/build.rs builds it with every paneflow-app build"
    status=1
    return
  fi
  local size
  size="$(wc -c < "$path" | tr -d '[:space:]')"
  total=$((total + size))
  local margin=$((hard - size))
  echo "$name = $size B (hard cap $hard B, margin $margin B)"
  if [ "$size" -gt "$hard" ]; then
    echo "::error::$name is $size B, over its hard cap of $hard B. Shrink it through smaller dependencies or a tighter release-min profile."
    status=1
    return
  fi
  if [ "$margin" -lt "$MARGIN_WARN" ]; then
    echo "::warning::$name is $size B, only $margin B under its hard cap of $hard B. A version bump alone moves a helper by about 1.5 KB: shrink it before the cap fails a build."
  fi
  if [ "$soft" -gt 0 ] && [ "$size" -gt "$soft" ]; then
    echo "::warning::$name is $size B, over its soft cap of $soft B."
  fi
}

measure paneflow-shim "$SHIM_HARD" "$SHIM_SOFT"
measure paneflow-ai-hook "$HOOK_HARD" "$HOOK_SOFT"
measure paneflow-mcp "$MCP_HARD" 0

total_margin=$((total_hard - total))
echo "embedded helpers = $total B (hard cap $total_hard B, margin $total_margin B)"
if [ "$total" -gt "$total_hard" ]; then
  echo "::error::the embedded helpers total $total B, over the hard cap of $total_hard B that src-app/build.rs enforces."
  status=1
elif [ "$status" -eq 0 ] && [ "$total_margin" -lt "$MARGIN_WARN" ]; then
  echo "::warning::the embedded helpers total $total B, only $total_margin B under the hard cap of $total_hard B."
fi

exit "$status"
