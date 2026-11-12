#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: sign-update-e2e-tarball.sh <new-assets-directory>" >&2
  echo "  signs <new-assets-directory>/paneflow-<Cargo.toml version>-x86_64.tar.gz with minisign, as the publish job will" >&2
  echo "  environment: MINISIGN_SECRET_KEY" >&2
  exit 2
fi
NEW_DIR="$1"
SCRIPTS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
TOOLS="${RUNNER_TEMP:-$(mktemp -d)}/minisign"

if [ -z "${MINISIGN_SECRET_KEY:+set}" ]; then
  echo "::error::MINISIGN_SECRET_KEY is required for auto-update e2e because OLD clients verify fail-closed."
  exit 1
fi

NEW_VERSION="$(awk -F'"' '/^version = / { print $2; exit }' Cargo.toml)"
tarball="${NEW_DIR}/paneflow-${NEW_VERSION}-x86_64.tar.gz"
[ -s "${tarball}" ] || {
  echo "::error::Expected NEW tarball not found before signing: ${tarball}"
  ls -la "${NEW_DIR}" || true
  exit 1
}

bash "$SCRIPTS/install-minisign.sh" "$TOOLS"
export PATH="$TOOLS:$PATH"

key_file="$(mktemp)"
trap 'rm -f "$key_file"' EXIT
printf '%s\n' "${MINISIGN_SECRET_KEY}" > "$key_file"
chmod 600 "$key_file"

base="$(basename "$tarball")"
minisign -S -s "$key_file" -m "$tarball" \
  -t "paneflow release artifact ${base}" >/dev/null
test -s "${tarball}.minisig"
ls -la "${NEW_DIR}"
