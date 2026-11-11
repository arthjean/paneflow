#!/usr/bin/env bash
set -euo pipefail

if (($# != 1)); then
  echo "usage: $0 <tag>" >&2
  echo "  fails when the GitHub Release of <tag> is already published, so that a re-run never replaces its assets;" >&2
  echo "  a missing release or a draft passes. Reads GITHUB_REPOSITORY" >&2
  exit 2
fi
tag="$1"
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"

if output="$(gh api "repos/$GITHUB_REPOSITORY/releases/tags/$tag" --jq '"\(.draft) \(.immutable) \(.html_url)"' 2>&1)"; then
  read -r draft immutable url <<<"$output"
  if [ "$draft" != "true" ]; then
    echo "::error::Release $tag is already published ($url, immutable: $immutable). A re-run never replaces published assets: cut a patch version instead (docs/release/runbook.md)."
    exit 1
  fi
  echo "release $tag exists as a draft; this run replaces its assets before publishing"
  exit 0
fi
if [[ "$output" == *"(HTTP 404)"* ]]; then
  echo "no published release for $tag yet"
  exit 0
fi
echo "::error::Could not read the release of $tag: $output"
exit 1
