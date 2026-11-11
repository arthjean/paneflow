#!/usr/bin/env bash
set -euo pipefail

if (($# < 4 || $# > 5)); then
  echo "usage: $0 <template> <version> <dmg-sha256> <output> [<published-cask>]" >&2
  echo "  renders the Homebrew cask from <template>, checks the rendered version and sha256 and its Ruby syntax," >&2
  echo "  and refuses a version lower than the one <published-cask> declares" >&2
  exit 2
fi
template="$1"
version="$2"
dmg_sha="$3"
output="$4"
published="${5:-}"

if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "::error::cask version '$version' is not MAJOR.MINOR.PATCH"
  exit 1
fi
if ! [[ "$dmg_sha" =~ ^[0-9a-f]{64}$ ]]; then
  echo "::error::'$dmg_sha' is not a SHA-256"
  exit 1
fi
[ -f "$template" ] || { echo "::error::cask template $template is missing"; exit 1; }

declared() {
  tr -d '\r' < "$2" | sed -nE "s/^  $1 \"([^\"]*)\"\$/\\1/p"
}

rendered="$(mktemp)"
trap 'rm -f "$rendered"' EXIT
tr -d '\r' < "$template" | sed \
  -e "s|^  version \".*\"\$|  version \"${version}\"|" \
  -e "s|^  sha256 \".*\"\$|  sha256 \"${dmg_sha}\"|" \
  > "$rendered"

found_versions="$(declared version "$rendered")"
found_shas="$(declared sha256 "$rendered")"
if [ "$found_versions" != "$version" ]; then
  echo "::error::Cask version is ${found_versions:-missing}, expected $version"
  exit 1
fi
if [ "$found_shas" != "$dmg_sha" ]; then
  echo "::error::Cask sha256 is ${found_shas:-missing}, expected $dmg_sha"
  exit 1
fi
if grep -qE '"0\.0\.0"|"0{64}"' "$rendered"; then
  echo "::error::the rendered cask still carries a template placeholder: $(grep -nE '"0\.0\.0"|"0{64}"' "$rendered" | head -1)"
  exit 1
fi

if ! command -v ruby >/dev/null; then
  echo "::error::ruby is required to check the cask syntax"
  exit 1
fi
if ! syntax="$(ruby --disable-gems -c "$rendered" 2>&1)"; then
  echo "::error::the rendered cask is not valid Ruby: $syntax"
  exit 1
fi

if [ -n "$published" ] && [ -f "$published" ]; then
  current="$(declared version "$published" | head -1)"
  if [ -n "$current" ] && [ "$current" != "$version" ]; then
    lowest="$(printf '%s\n%s\n' "$current" "$version" | sort -V | head -1)"
    if [ "$lowest" = "$version" ]; then
      echo "::error::Cask version $version is lower than the $current the tap already publishes; refusing a downgrade"
      exit 1
    fi
  fi
  echo "the tap publishes ${current:-no version}, rendering $version"
fi

cp "$rendered" "$output"
echo "rendered $output: version $version, sha256 $dmg_sha, $syntax"
