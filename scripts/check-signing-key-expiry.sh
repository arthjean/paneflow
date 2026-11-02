#!/usr/bin/env bash
set -euo pipefail

if (($# != 2)); then
  echo "usage: $0 <armored-public-key> <minimum-days-left>" >&2
  exit 2
fi

key_file="$1"
min_days="$2"

if ! [[ "$min_days" =~ ^[0-9]+$ ]]; then
  echo "::error::minimum days must be a non-negative integer, got '$min_days'" >&2
  exit 2
fi

home="$(mktemp -d)"
trap 'rm -rf "$home"' EXIT

if ! listing="$(GNUPGHOME="$home" gpg --batch --quiet --show-keys --with-colons "$key_file")"; then
  echo "::error::could not read $key_file as an OpenPGP public key"
  exit 2
fi
expires_epoch="$(awk -F: '$1 == "pub" { print $7; exit }' <<<"$listing")"
if [ -z "$expires_epoch" ]; then
  echo "::error::$key_file has no expiry date on its primary key"
  exit 2
fi
if ! [[ "$expires_epoch" =~ ^[0-9]+$ ]]; then
  echo "::error::could not read the expiry of $key_file (got '$expires_epoch')"
  exit 2
fi

expires="$(date -u -d "@$expires_epoch" +%Y-%m-%d)"
days_left=$(((expires_epoch - $(date -u +%s)) / 86400))

if [ -n "${GITHUB_OUTPUT:-}" ]; then
  {
    echo "expires=$expires"
    echo "days_left=$days_left"
  } >> "$GITHUB_OUTPUT"
fi

if ((days_left < min_days)); then
  echo "::error::the repository signing key $key_file expires on $expires, in $days_left days (threshold $min_days)"
  exit 1
fi
echo "the repository signing key $key_file expires on $expires, in $days_left days"
