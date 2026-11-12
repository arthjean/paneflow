#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 0 ]; then
  echo "usage: detect-windows-signing.sh" >&2
  echo "  writes signing_available=true|false to $GITHUB_OUTPUT from the six AZURE_* variables" >&2
  exit 2
fi
GITHUB_REF="${GITHUB_REF:-this ref}"
GITHUB_OUTPUT="${GITHUB_OUTPUT:-/dev/stdout}"

if [ -n "${AZURE_TENANT_ID:+set}" ] \
   && [ -n "${AZURE_CLIENT_ID:+set}" ] \
   && [ -n "${AZURE_CLIENT_SECRET:+set}" ] \
   && [ -n "${AZURE_TRUSTED_SIGNING_ENDPOINT:+set}" ] \
   && [ -n "${AZURE_TRUSTED_SIGNING_ACCOUNT:+set}" ] \
   && [ -n "${AZURE_TRUSTED_SIGNING_CERT_PROFILE:+set}" ]; then
    echo "All 6 Azure Trusted Signing secrets are present - will sign the EXE, then produce + sign + verify MSI."
    echo "signing_available=true" >> "$GITHUB_OUTPUT"
else
    echo "::warning title=Windows signing skipped::Signing secrets unavailable on ${GITHUB_REF}: building unsigned dry-run artifacts"
    echo "::warning title=Windows MSI packaging skipped::One or more AZURE_* secrets are missing - skipping Preflight WiX / Install cargo-wix / Produce MSI / Sign / Verify / Stage. Build/test/clippy still run for cross-platform validation. See docs/release/windows-signing.md to enable."
    echo "signing_available=false" >> "$GITHUB_OUTPUT"
fi
