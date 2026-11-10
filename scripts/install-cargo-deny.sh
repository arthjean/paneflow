#!/usr/bin/env bash
set -euo pipefail

DENY_VERSION="0.19.8"
DENY_SHA256="70e769ae3872e34d45132b17040859175e11401dc12dddb0303e0b8c7d088f3f"
DENY_TRIPLE="x86_64-unknown-linux-musl"

host="$(uname -s)-$(uname -m)"
if [ "$host" != "Linux-x86_64" ]; then
  echo "::error::scripts/install-cargo-deny.sh pins the $DENY_TRIPLE binary only; this host is $host"
  exit 1
fi

name="cargo-deny-$DENY_VERSION-$DENY_TRIPLE"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

curl --fail --silent --show-error --location \
  --retry 5 --retry-all-errors --connect-timeout 30 --max-time 300 \
  --output "$work/$name.tar.gz" \
  "https://github.com/EmbarkStudios/cargo-deny/releases/download/$DENY_VERSION/$name.tar.gz"

if ! echo "$DENY_SHA256  $work/$name.tar.gz" | sha256sum --check --status -; then
  echo "::error::$name.tar.gz is $(sha256sum "$work/$name.tar.gz" | cut -d' ' -f1), expected $DENY_SHA256"
  exit 1
fi

tar -xzf "$work/$name.tar.gz" -C "$work"
bin_dir="${CARGO_HOME:-$HOME/.cargo}/bin"
mkdir -p "$bin_dir"
install -m 0755 "$work/$name/cargo-deny" "$bin_dir/cargo-deny"

installed="$("$bin_dir/cargo-deny" --version)"
if [ "$installed" != "cargo-deny $DENY_VERSION" ]; then
  echo "::error::installed '$installed', expected 'cargo-deny $DENY_VERSION'"
  exit 1
fi
echo "$installed ($DENY_TRIPLE, sha256 $DENY_SHA256)"
