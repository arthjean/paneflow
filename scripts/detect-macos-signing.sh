#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 0 ]; then
  echo "usage: detect-macos-signing.sh" >&2
  echo "  writes signing_available=true|false to $GITHUB_OUTPUT from the five APPLE_* variables; a release (RELEASE_MODE=release) fails without them" >&2
  exit 2
fi
RELEASE_MODE="${RELEASE_MODE:-release}"
GITHUB_REF="${GITHUB_REF:-this ref}"
GITHUB_OUTPUT="${GITHUB_OUTPUT:-/dev/stdout}"

if [ -n "${APPLE_DEVELOPER_CERT_P12:+set}" ] \
   && [ -n "${APPLE_DEVELOPER_CERT_PASSWORD:+set}" ] \
   && [ -n "${APPLE_ID:+set}" ] \
   && [ -n "${APPLE_APP_SPECIFIC_PASSWORD:+set}" ] \
   && [ -n "${APPLE_TEAM_ID:+set}" ]; then
    echo "All 5 Apple signing secrets are present - will sign + notarize."
    echo "signing_available=true" >> "$GITHUB_OUTPUT"
else
    if [ "${RELEASE_MODE}" = "release" ]; then
        echo "::error title=macOS signing required::One or more APPLE_* secrets are missing. Tag-push releases must sign and notarize the macOS artifact."
        exit 1
    fi
    echo "::warning title=macOS signing skipped::Signing secrets unavailable on ${GITHUB_REF}: building unsigned dry-run artifacts"
    echo "::warning title=macOS signing skipped::One or more APPLE_* secrets are missing - emitting an UNSIGNED .app. See docs/release/macos-signing.md."
    echo "signing_available=false" >> "$GITHUB_OUTPUT"
fi
