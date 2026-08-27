#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
MANIFEST="$ROOT/native/libghostty/manifest.toml"
SOURCE_DIR="${PANEFLOW_GHOSTTY_SOURCE_DIR:-}"
BINDGEN_VERSION="0.72.1"

manifest_string() {
  sed -n "s/^$1 = \"\(.*\)\"$/\1/p" "$MANIFEST"
}

[[ -n "$SOURCE_DIR" ]] || {
  echo "PANEFLOW_GHOSTTY_SOURCE_DIR must point to the pinned Ghostty checkout" >&2
  exit 1
}
command -v bindgen >/dev/null || {
  echo "bindgen-cli $BINDGEN_VERSION is required" >&2
  exit 1
}
ACTUAL_BINDGEN="$(bindgen --version | awk '{print $2}')"
[[ "$ACTUAL_BINDGEN" == "$BINDGEN_VERSION" ]] || {
  echo "bindgen-cli $BINDGEN_VERSION is required, found $ACTUAL_BINDGEN" >&2
  exit 1
}

EXPECTED_SHA="$(manifest_string source_sha)"
ACTUAL_SHA="$(git -C "$SOURCE_DIR" rev-parse HEAD)"
[[ "$ACTUAL_SHA" == "$EXPECTED_SHA" ]] || {
  echo "Ghostty source mismatch: expected $EXPECTED_SHA, got $ACTUAL_SHA" >&2
  exit 1
}
SOURCE_STATUS="$(git -C "$SOURCE_DIR" status --porcelain --untracked-files=all)"
[[ -z "$SOURCE_STATUS" ]] || {
  echo "Ghostty source must be a clean checkout of $EXPECTED_SHA" >&2
  printf '%s\n' "$SOURCE_STATUS" >&2
  exit 1
}

HEADER="$SOURCE_DIR/$(manifest_string header_path)"
EXPECTED_HEADER_SHA="$(manifest_string header_sha256)"
ACTUAL_HEADER_SHA="$(sha256sum "$HEADER" | awk '{print $1}')"
[[ "$ACTUAL_HEADER_SHA" == "$EXPECTED_HEADER_SHA" ]] || {
  echo "Ghostty header checksum mismatch: expected $EXPECTED_HEADER_SHA, got $ACTUAL_HEADER_SHA" >&2
  exit 1
}
OUTPUT="$(mktemp)"
trap 'rm -f "$OUTPUT"' EXIT
bindgen \
  --use-core \
  --no-doc-comments \
  --allowlist-function '^ghostty_.*' \
  --allowlist-type '^Ghostty.*' \
  --allowlist-var '^(GHOSTTY|Ghostty).*' \
  --formatter rustfmt \
  --rust-target 1.82 \
  --output "$OUTPUT" \
  "$HEADER" \
  -- \
  -I"$SOURCE_DIR/include"

# Bindgen 0.72 qualifies core from the crate root. Keep the reviewed bindings
# stable across the generator upgrade without changing their Rust semantics.
sed -i 's/::core::ffi::/core::ffi::/g' "$OUTPUT"

# Normalize the enum type aliases to `c_int`.
#
# bindgen takes the alias from whatever libclang reports as the enum's
# underlying type, and that answer moved with the compiler: clang 14 (the
# ubuntu-22.04 gate) and clang 19 both report `unsigned int` for an
# enumeration whose enumerators are all non-negative, while clang 20 and
# later report `int`. Without this the file regenerates differently on the
# gate than on a current desktop toolchain, which is exactly the diff that
# landed with the f2d5758f re-pin.
#
# `int` is the reviewed choice and the ABI is the same either way: every
# Ghostty enum tops out at its `MAX_VALUE = 2147483647` sentinel, so the
# values fit in a signed 32-bit integer and the C calling convention passes
# both types identically. An enumerator above `i32::MAX` would fail to
# compile here rather than silently change the alias.
sed -i 's/^pub type \(Ghostty[A-Za-z0-9_]*\) = core::ffi::c_uint;$/pub type \1 = core::ffi::c_int;/' "$OUTPUT"
rustfmt --edition 2024 "$OUTPUT"

if ! cmp -s "$OUTPUT" "$ROOT/$(manifest_string bindings_path)"; then
  diff -u "$ROOT/$(manifest_string bindings_path)" "$OUTPUT" || true
  echo "libghostty bindings differ from the pinned header; review and commit the regeneration" >&2
  exit 1
fi
echo "libghostty bindings match the pinned header and bindgen $BINDGEN_VERSION"
