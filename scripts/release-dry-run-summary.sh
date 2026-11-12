#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: release-dry-run-summary.sh <tag> <built-ref-description> <asset-directory>" >&2
  echo "  appends the table of dry-run assets, sizes and SHA-256 to $GITHUB_STEP_SUMMARY" >&2
  exit 2
fi
DRYRUN_TAG="$1"
DRYRUN_REF="$2"
ASSET_DIR="$3"
GITHUB_STEP_SUMMARY="${GITHUB_STEP_SUMMARY:-/dev/stdout}"

{
  echo "## Dry-run artifacts (${DRYRUN_TAG})"
  echo
  echo "Built from \`${DRYRUN_REF}\`. Workflow artifacts have"
  echo "7-day retention - download them from the Actions UI"
  echo "sidebar to inspect before cutting the real tag."
  echo
  echo "| Asset | Size | SHA-256 |"
  echo "|---|---:|---|"
} >> "$GITHUB_STEP_SUMMARY"
while IFS= read -r -d '' f; do
  [ -d "$f" ] && continue
  case "$f" in
    *.sha256) continue ;;
  esac
  name="$(basename "$f")"
  size="$(stat -c '%s' "$f")"
  hash="$(sha256sum "$f" | awk '{print $1}')"
  printf '| `%s` | %s | `%s` |\n' "$name" "$size" "$hash"
done < <(find "$ASSET_DIR" -type f -print0 | sort -z) \
  >> "$GITHUB_STEP_SUMMARY"
