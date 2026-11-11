#!/usr/bin/env bash
set -euo pipefail

MINISIGN_VERSION=0.12
LINUX_ARCHIVE_SHA256=9a599b48ba6eb7b1e80f12f36b94ceca7c00b7a5173c95c3efc88d9822957e73
MACOS_ARCHIVE_SHA256=89000b19535765f9cffc65a65d64a820f433ef6db8020667f7570e06bf6aac63

if (($# != 1)); then
  echo "usage: $0 <install-dir>" >&2
  echo "  installs minisign $MINISIGN_VERSION as <install-dir>/minisign, checked against a pinned SHA-256," >&2
  echo "  and appends <install-dir> to \$GITHUB_PATH when it is set" >&2
  exit 2
fi
install_dir="$1"

sha256_of() {
  if command -v sha256sum >/dev/null; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

case "$(uname -s)" in
  Linux)
    archive="minisign-$MINISIGN_VERSION-linux.tar.gz"
    expected="$LINUX_ARCHIVE_SHA256"
    case "$(uname -m)" in
      x86_64 | amd64) member="minisign-linux/x86_64/minisign" ;;
      aarch64 | arm64) member="minisign-linux/aarch64/minisign" ;;
      *) echo "::error::minisign $MINISIGN_VERSION ships no Linux binary for $(uname -m)" >&2; exit 1 ;;
    esac
    ;;
  Darwin)
    archive="minisign-$MINISIGN_VERSION-macos.zip"
    expected="$MACOS_ARCHIVE_SHA256"
    member="minisign"
    [ "$(uname -m)" = arm64 ] || { echo "::error::minisign $MINISIGN_VERSION ships an arm64-only macOS binary, this host is $(uname -m)" >&2; exit 1; }
    ;;
  *)
    echo "::error::install-minisign.sh supports Linux and macOS, not $(uname -s)" >&2
    exit 1
    ;;
esac

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

curl -sSfL --retry 5 --retry-connrefused --max-time 120 -o "$work/$archive" \
  "https://github.com/jedisct1/minisign/releases/download/$MINISIGN_VERSION/$archive"
actual="$(sha256_of "$work/$archive")"
if [ "$actual" != "$expected" ]; then
  echo "::error::$archive checksum mismatch: expected $expected, got $actual" >&2
  exit 1
fi

case "$archive" in
  *.tar.gz) tar --warning=no-unknown-keyword -xzf "$work/$archive" -C "$work" "$member" ;;
  *.zip) unzip -q -o "$work/$archive" "$member" -d "$work" ;;
esac

mkdir -p "$install_dir"
install -m 0755 "$work/$member" "$install_dir/minisign"
"$install_dir/minisign" -v
if [ -n "${GITHUB_PATH:-}" ]; then
  printf '%s\n' "$install_dir" >> "$GITHUB_PATH"
fi
