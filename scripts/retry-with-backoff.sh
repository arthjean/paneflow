#!/usr/bin/env bash
set -euo pipefail

ATTEMPTS="${RETRY_ATTEMPTS:-3}"
BACKOFF="${RETRY_BACKOFF_SECONDS:-15}"

if (($# < 1)); then
  echo "usage: $0 <command> [args...]" >&2
  exit 2
fi

output="$(mktemp)"
trap 'rm -f "$output"' EXIT

for ((attempt = 1; attempt <= ATTEMPTS; attempt++)); do
  status=0
  "$@" >"$output" || status=$?
  if ((status == 0)); then
    cat "$output"
    if ((attempt > 1)); then
      echo "succeeded on attempt $attempt/$ATTEMPTS: $1" >&2
    fi
    exit 0
  fi
  cat "$output" >&2
  if ((attempt == ATTEMPTS)); then
    echo "::error::$1 still failing after $ATTEMPTS attempts (exit $status)" >&2
    exit "$status"
  fi
  delay=$((BACKOFF * attempt))
  echo "::warning::attempt $attempt/$ATTEMPTS of $1 failed (exit $status); retrying in ${delay}s" >&2
  sleep "$delay"
done
