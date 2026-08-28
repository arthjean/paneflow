#!/usr/bin/env bash
# Re-run a command that failed on a transient network error, and only then.
#
# Ghostty's `build.zig.zon` at the pinned source SHA declares 16 dependencies
# served by two hosts (`deps.files.ghostty.org` and `github.com`). The native
# lanes deliberately keep no object cache, because the canonical source and
# cache prefixes are part of the reproducibility contract, and
# `--verify-reproducible` runs a second clean build in a fresh temporary cache.
# Every native job therefore fetches all 16 dependencies cold, twice, over two
# single-vendor hosts, with no retry anywhere in the chain. A DNS blip there
# has already turned `libghostty Linux` and, more expensively, `release` red.
#
# The retry is deliberately narrow. A failure whose output carries none of the
# transient signatures below fails immediately, so a genuine archive hash
# mismatch still costs one rebuild rather than three.
#
# Usage: scripts/ci-retry-network.sh <command> [args...]
set -euo pipefail

ATTEMPTS="${CI_RETRY_ATTEMPTS:-3}"
BACKOFF="${CI_RETRY_BACKOFF_SECONDS:-20}"

# Signatures emitted by Zig's package fetcher, curl, and git when the transport
# fails rather than the build. Matched on message text because the fetch
# happens several layers below this script and never surfaces a distinct code.
TRANSIENT='unable to connect to server|TemporaryNameServerFailure|UnknownHostName|HostLacksNetworkAddresses|NameServerFailure|ConnectionRefused|ConnectionResetByPeer|ConnectionTimedOut|NetworkUnreachable|TlsInitializationFailed|unexpected EOF|TemporaryFailure|error: unable to fetch|Could not resolve host|Connection reset by peer|Operation timed out|502 Bad Gateway|503 Service Unavailable'

if (($# < 1)); then
  echo "usage: $0 <command> [args...]" >&2
  exit 2
fi

log="$(mktemp)"
trap 'rm -f "$log"' EXIT

for ((attempt = 1; attempt <= ATTEMPTS; attempt++)); do
  echo "::group::attempt $attempt/$ATTEMPTS: $*"
  # A pipe rather than process substitution: `pipefail` gives us the command's
  # own status, and the shell waits for `tee` to flush before `grep` reads it.
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
