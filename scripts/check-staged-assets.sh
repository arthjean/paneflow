#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 0 ]; then
  echo "usage: check-staged-assets.sh" >&2
  echo "  fails when release-assets/ holds no file, except in a dry-run (RELEASE_MODE=dry-run), where an unsigned leg stages nothing" >&2
  exit 2
fi
RELEASE_MODE="${RELEASE_MODE:-release}"
RUNNER_OS="${RUNNER_OS:-$(uname -s)}"

if [ ! -d release-assets ] || ! find release-assets -maxdepth 1 -type f | grep -q .; then
  if [ "${RELEASE_MODE}" = "dry-run" ]; then
    echo "::warning::No release artifacts were staged for ${RUNNER_OS}. Dry-run upload will be empty."
    exit 0
  fi
  echo "::error::No release artifacts were staged for ${RUNNER_OS} in release mode."
  exit 1
fi
