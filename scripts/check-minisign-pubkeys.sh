#!/usr/bin/env bash
set -euo pipefail

MINISIGN_PUBKEY_BASE64_LENGTH=56
MINISIGN_ED25519_ALGORITHM="Ed"

trim() {
  local value="$1"
  value="${value#"${value%%[![:space:]]*}"}"
  value="${value%"${value##*[![:space:]]}"}"
  printf '%s' "$value"
}

check_slot() {
  local name="$1"
  local requirement="$2"
  local value
  value="$(trim "${!name:-}")"
  if [ -z "$value" ]; then
    if [ "$requirement" = "required" ]; then
      echo "::error::$name is empty. Every shipped binary verifies updates fail-closed against this key, so a release without it cannot self-update."
      return 1
    fi
    echo "$name is empty (optional rotation slot)"
    return 0
  fi
  if [ "${#value}" -ne "$MINISIGN_PUBKEY_BASE64_LENGTH" ] || ! [[ "$value" =~ ^[A-Za-z0-9+/]+$ ]]; then
    echo "::error::$name is not a minisign public key: expected the $MINISIGN_PUBKEY_BASE64_LENGTH-character base64 line of a minisign .pub file."
    return 1
  fi
  local algorithm
  algorithm="$(printf '%s' "$value" | base64 -d 2>/dev/null | head -c 2 || true)"
  if [ "$algorithm" != "$MINISIGN_ED25519_ALGORITHM" ]; then
    echo "::error::$name does not decode to a minisign Ed25519 public key."
    return 1
  fi
  echo "$name is a well-formed minisign public key"
}

status=0
check_slot PANEFLOW_MINISIGN_PUBKEY required || status=1
check_slot PANEFLOW_MINISIGN_PUBKEY_NEXT optional || status=1
exit "$status"
