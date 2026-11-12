#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: summarize-unsigned-macos.sh <arch> <tag>" >&2
  echo "  appends the unsigned macOS build notice to $GITHUB_STEP_SUMMARY" >&2
  exit 2
fi
ARCH="$1"
TAG="$2"
GITHUB_STEP_SUMMARY="${GITHUB_STEP_SUMMARY:-/dev/stdout}"

{
    echo "## ⚠️ Unsigned macOS build (${ARCH}, ${TAG})"
    echo
    echo "One or more APPLE_* secrets were missing, so this leg shipped an"
    echo "**unsigned, un-notarized** \`PaneFlow.app\`. macOS Gatekeeper will"
    echo "block first-launch with the \"can't be opened\" prompt."
    echo
    echo "Required secrets: \`APPLE_DEVELOPER_CERT_P12\`, "
    echo "\`APPLE_DEVELOPER_CERT_PASSWORD\`, \`APPLE_ID\`,"
    echo "\`APPLE_APP_SPECIFIC_PASSWORD\`, \`APPLE_TEAM_ID\`."
    echo
    echo "Onboarding runbook: \`docs/release/macos-signing.md\`."
} >> "$GITHUB_STEP_SUMMARY"
