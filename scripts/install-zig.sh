#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
ZIG_MINISIGN_PUBKEY="RWSGOq2NVecA2UPNdBUZykf1CCb147pkmdtYxgb3Ti+JO/wCYvhbAb/U"
ZIG_DOWNLOAD_BASE="https://ziglang.org/download"

if (($# < 1 || $# > 2)); then
  echo "usage: $0 <zig-version> [<install-dir>]" >&2
  echo "  downloads the official Zig distribution for this host, verifies its minisign signature" >&2
  echo "  against the ziglang.org key, and installs it in <install-dir>, by default" >&2
  echo "  \$RUNNER_TOOL_CACHE/zig/<version>/<x64|arm64>; appends that directory to \$GITHUB_PATH when it is set" >&2
  exit 2
fi
version="$1"
if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "::error::Zig version '$version' is not MAJOR.MINOR.PATCH" >&2
  exit 1
fi

case "$(uname -s)" in
  Linux) os=linux ;;
  Darwin) os=macos ;;
  *) echo "::error::install-zig.sh supports Linux and macOS, not $(uname -s)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64; tool_arch=x64 ;;
  aarch64 | arm64) arch=aarch64; tool_arch=arm64 ;;
  *) echo "::error::no Zig distribution for $(uname -m)" >&2; exit 1 ;;
esac

if (($# == 2)); then
  install_dir="$2"
else
  install_dir="${RUNNER_TOOL_CACHE:?RUNNER_TOOL_CACHE is unset; pass an install directory}/zig/$version/$tool_arch"
fi
if [ -e "$install_dir" ]; then
  echo "::error::refusing to replace an existing Zig install at $install_dir" >&2
  exit 1
fi

name="zig-$arch-$os-$version"
tarball="$name.tar.xz"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

if command -v minisign >/dev/null; then
  minisign="$(command -v minisign)"
else
  GITHUB_PATH="" bash "$ROOT/scripts/install-minisign.sh" "$work/minisign-bin" >/dev/null
  minisign="$work/minisign-bin/minisign"
fi

for file in "$tarball" "$tarball.minisig"; do
  curl -sSfL --retry 5 --retry-connrefused --max-time 300 -o "$work/$file" "$ZIG_DOWNLOAD_BASE/$version/$file"
done

trusted_comment="$("$minisign" -V -Q -P "$ZIG_MINISIGN_PUBKEY" -m "$work/$tarball" -x "$work/$tarball.minisig")" || {
  echo "::error::$tarball does not carry a valid ziglang.org minisign signature" >&2
  exit 1
}
if ! [[ "$trusted_comment" =~ ^timestamp:[0-9]+[[:space:]]+file:([^[:space:]]+)([[:space:]]+hashed)?$ ]] || [ "${BASH_REMATCH[1]}" != "$tarball" ]; then
  echo "::error::the signature of $tarball was made for another file: trusted comment '$trusted_comment'" >&2
  exit 1
fi

tar -xJf "$work/$tarball" -C "$work"
mkdir -p "$(dirname "$install_dir")"
mv "$work/$name" "$install_dir"

installed="$("$install_dir/zig" version)"
if [ "$installed" != "$version" ]; then
  echo "::error::$install_dir/zig reports version $installed, expected $version" >&2
  exit 1
fi
echo "Zig $version installed at $install_dir, signature verified ($trusted_comment)"
if [ -n "${GITHUB_PATH:-}" ]; then
  printf '%s\n' "$install_dir" >> "$GITHUB_PATH"
fi
