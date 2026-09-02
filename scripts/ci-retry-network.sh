#!/usr/bin/env bash
set -euo pipefail

ATTEMPTS="${CI_RETRY_ATTEMPTS:-3}"
BACKOFF="${CI_RETRY_BACKOFF_SECONDS:-20}"

TRANSIENT='unable to connect to server|TemporaryNameServerFailure|UnknownHostName|HostLacksNetworkAddresses|NameServerFailure|ConnectionRefused|ConnectionResetByPeer|ConnectionTimedOut|NetworkUnreachable|TlsInitializationFailed|unexpected EOF|TemporaryFailure|error: unable to fetch|Could not resolve host|Connection reset by peer|Operation timed out|502 Bad Gateway|503 Service Unavailable'

if (($# < 1)); then
  echo "usage: $0 <command> [args...]" >&2
  exit 2
fi

log="$(mktemp)"
trap 'rm -f "$log"' EXIT

for ((attempt = 1; attempt <= ATTEMPTS; attempt++)); do
  echo "::group::attempt $attempt/$ATTEMPTS: $*"
  status=0
  "$@" 2>&1 | tee "$log" || status=$?
  echo "::endgroup::"

  if ((status == 0)); then
    if ((attempt > 1)); then
      echo "succeeded on attempt $attempt after a transient network failure"
    fi
    exit 0
  fi

  if ! grep -qE "$TRANSIENT" "$log"; then
    echo "::error::command failed with exit $status and no transient network signature; not retrying"
    exit "$status"
  fi

  if ((attempt == ATTEMPTS)); then
    echo "::error::still failing on a transient network error after $ATTEMPTS attempts"
    exit "$status"
  fi

  delay=$((BACKOFF * attempt))
  echo "::warning::transient network failure (exit $status); retrying in ${delay}s"
  sleep "$delay"
done
