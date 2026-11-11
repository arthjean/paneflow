#!/usr/bin/env bash
set -euo pipefail

TRUSTED_COMMENT_PREFIX="paneflow release artifact"

if (($# < 1)); then
  echo "usage: $0 <asset>..." >&2
  echo "  verifies <asset>.minisig for every asset against PANEFLOW_MINISIGN_PUBKEY, or PANEFLOW_MINISIGN_PUBKEY_NEXT" >&2
  echo "  when it is set, the keys embedded in every shipped binary; the trusted comment must name the asset" >&2
  exit 2
fi

trim() {
  local value="$1"
  value="${value#"${value%%[![:space:]]*}"}"
  value="${value%"${value##*[![:space:]]}"}"
  printf '%s' "$value"
}

keys=()
current="$(trim "${PANEFLOW_MINISIGN_PUBKEY:-}")"
next="$(trim "${PANEFLOW_MINISIGN_PUBKEY_NEXT:-}")"
if [ -z "$current" ]; then
  echo "::error::PANEFLOW_MINISIGN_PUBKEY is empty: there is no embedded key to verify the release assets against"
  exit 1
fi
keys+=("$current")
[ -z "$next" ] || keys+=("$next")

if ! command -v minisign >/dev/null; then
  echo "::error::minisign is not installed; run scripts/install-minisign.sh first"
  exit 1
fi

status=0
for asset in "$@"; do
  name="$(basename "$asset")"
  if [ ! -f "$asset" ]; then
    echo "::error::Signature check failed for $name: the asset is missing, not publishing"
    status=1
    continue
  fi
  if [ ! -f "$asset.minisig" ]; then
    echo "::error::Signature check failed for $name: $name.minisig is missing, not publishing"
    status=1
    continue
  fi
  verified=""
  for key in "${keys[@]}"; do
    if comment="$(minisign -V -Q -P "$key" -m "$asset" -x "$asset.minisig" 2>/dev/null)"; then
      verified="$comment"
      break
    fi
  done
  if [ -z "$verified" ]; then
    echo "::error::Signature check failed for $name: no embedded key verifies $name.minisig, not publishing"
    status=1
    continue
  fi
  if [ "$verified" != "$TRUSTED_COMMENT_PREFIX $name" ]; then
    echo "::error::Signature check failed for $name: the signature was made for '$verified', not publishing"
    status=1
    continue
  fi
  echo "verified: $name"
done

if ((status == 0)); then
  echo "$# asset(s) carry a valid minisign signature from an embedded key"
fi
exit "$status"
