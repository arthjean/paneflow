#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: summarize-skipped-msi.sh <arch> <tag>" >&2
  echo "  appends the skipped Windows MSI notice to $GITHUB_STEP_SUMMARY" >&2
  exit 2
fi
ARCH="$1"
TAG="$2"
GITHUB_STEP_SUMMARY="${GITHUB_STEP_SUMMARY:-/dev/stdout}"

{
    echo "## ℹ️ Windows MSI packaging skipped (${ARCH}, ${TAG})"
    echo
    echo "One or more AZURE_* secrets were missing, so this leg ran"
    echo "**only the build/test/clippy validation** for Windows and produced"
    echo "no \`.msi\` artifact. This is intentional: we don't ship"
    echo "unsigned Windows installers."
    echo
    echo "Required secrets: \`AZURE_TENANT_ID\`, \`AZURE_CLIENT_ID\`,"
    echo "\`AZURE_CLIENT_SECRET\`, \`AZURE_TRUSTED_SIGNING_ENDPOINT\`,"
    echo "\`AZURE_TRUSTED_SIGNING_ACCOUNT\`, \`AZURE_TRUSTED_SIGNING_CERT_PROFILE\`."
    echo
    echo "Once populated, the MSI chain re-activates automatically"
    echo "with no other workflow change. Onboarding runbook:"
    echo "\`docs/release/windows-signing.md\`."
} >> "$GITHUB_STEP_SUMMARY"
