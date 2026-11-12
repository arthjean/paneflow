#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: sign-release-assets.sh <asset-directory>" >&2
  echo "  writes a .minisig next to every file of the directory, except the .sha256, .minisig and .zsync files" >&2
  echo "  environment: MINISIGN_SECRET_KEY, the unencrypted minisign secret key" >&2
  exit 2
fi
ASSET_DIR="$1"
SCRIPTS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
TOOLS="${RUNNER_TEMP:-$(mktemp -d)}/minisign"

if [ -z "${MINISIGN_SECRET_KEY:+set}" ]; then
  echo "::error::MINISIGN_SECRET_KEY is required. Clients with an embedded key refuse unsigned self-update assets."
  exit 1
fi
bash "$SCRIPTS/install-minisign.sh" "$TOOLS"
export PATH="$TOOLS:$PATH"
key_file="$(mktemp)"
trap 'rm -f "$key_file"' EXIT
printf '%s\n' "${MINISIGN_SECRET_KEY}" > "$key_file"
chmod 600 "$key_file"
shopt -s nullglob
for f in "$ASSET_DIR"/*; do
  case "$f" in
    *.sha256|*.minisig|*.zsync) continue ;;
  esac
  [ -f "$f" ] || continue
  base="$(basename "$f")"
  minisign -S -s "$key_file" -m "$f" \
    -t "paneflow release artifact ${base}" >/dev/null
  echo "signed: ${base} -> ${base}.minisig"
done
