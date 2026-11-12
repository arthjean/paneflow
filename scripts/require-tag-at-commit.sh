#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: require-tag-at-commit.sh <tag> <commit-sha>" >&2
  echo "  fails when <tag> of $GITHUB_REPOSITORY no longer points at <commit-sha>" >&2
  exit 2
fi
RELEASE_TAG="$1"
RELEASE_SHA="$2"
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required, as owner/name}"

listing="$(git ls-remote "https://github.com/$GITHUB_REPOSITORY.git" "refs/tags/$RELEASE_TAG" "refs/tags/$RELEASE_TAG^{}")"
tagged="$(awk -v r="refs/tags/$RELEASE_TAG^{}" '$2 == r { print $1; exit }' <<< "$listing")"
if [ -z "$tagged" ]; then
  tagged="$(awk -v r="refs/tags/$RELEASE_TAG" '$2 == r { print $1; exit }' <<< "$listing")"
fi
if [ "$tagged" != "$RELEASE_SHA" ]; then
  echo "::error::Tag $RELEASE_TAG now points to ${tagged:-nothing}, this run built $RELEASE_SHA. Re-run the release"
  exit 1
fi
echo "$RELEASE_TAG still points to $RELEASE_SHA"
