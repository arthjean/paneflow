#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: resolve-previous-release.sh <current-tag>" >&2
  echo "  writes old_tag and old_version, the newest published stable release other than <current-tag>, to $GITHUB_OUTPUT; old_version is empty when there is none" >&2
  echo "  environment: GH_TOKEN, and GH_REPO outside a checkout" >&2
  exit 2
fi
RELEASE_TAG="$1"
GITHUB_OUTPUT="${GITHUB_OUTPUT:-/dev/stdout}"

CURRENT_TAG="${RELEASE_TAG:-}"
PREV_TAG="$(gh release list \
    --exclude-pre-releases \
    --exclude-drafts \
    --limit 5 \
    --json tagName,createdAt \
    --jq 'sort_by(.createdAt) | reverse | .[] | .tagName' \
  | grep -v "^${CURRENT_TAG}$" \
  | head -n1)"
if [ -z "${PREV_TAG}" ]; then
  echo "No previous stable release found - harness will self-bootstrap to no-op"
  echo "old_version=" >> "$GITHUB_OUTPUT"
  exit 0
fi
OLD_VERSION="${PREV_TAG#v}"
echo "Previous stable: ${PREV_TAG} (OLD_VERSION=${OLD_VERSION})"
echo "old_version=${OLD_VERSION}" >> "$GITHUB_OUTPUT"
echo "old_tag=${PREV_TAG}" >> "$GITHUB_OUTPUT"
