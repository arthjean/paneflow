#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: run-update-e2e.sh <new-assets-directory> <old-assets-directory>" >&2
  echo "  runs scripts/test-update-e2e.sh from the old x86_64 tarball to the new one" >&2
  echo "  environment: OLD_VERSION, the version of the old tarball" >&2
  exit 2
fi
NEW_DIR="$1"
OLD_DIR="$2"
SCRIPTS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"

NEW_VERSION="$(awk -F'"' '/^version = / { print $2; exit }' Cargo.toml)"
echo "NEW_VERSION=${NEW_VERSION}"
E2E_NEW_TARBALL="${NEW_DIR}/paneflow-${NEW_VERSION}-x86_64.tar.gz"
[ -s "${E2E_NEW_TARBALL}" ] || {
  echo "::error::Expected NEW tarball not found in build artifact: ${E2E_NEW_TARBALL}"
  ls -la "${NEW_DIR}" || true
  exit 1
}
E2E_OLD_TARBALL="$(find "${OLD_DIR}" -maxdepth 1 -name 'paneflow-*-x86_64.tar.gz' | head -n1)"
[ -n "${E2E_OLD_TARBALL}" ] && [ -s "${E2E_OLD_TARBALL}" ] || {
  echo "::error::Expected OLD tarball not found under ${OLD_DIR}"
  ls -la "${OLD_DIR}" || true
  exit 1
}
export E2E_NEW_TARBALL E2E_OLD_TARBALL
bash "$SCRIPTS/test-update-e2e.sh"
