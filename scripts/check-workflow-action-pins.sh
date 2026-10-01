#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
RELEASE_WORKFLOWS=(
  ".github/workflows/release.yml"
  ".github/workflows/audit.yml"
  ".github/workflows/repo_publish.yml"
  ".github/workflows/update_cask.yml"
)

if [ "$#" -gt 0 ]; then
  files=("$@")
else
  files=()
  for workflow in "${RELEASE_WORKFLOWS[@]}"; do
    files+=("$ROOT/$workflow")
  done
  for composite in "$ROOT"/.github/actions/*/action.yml; do
    files+=("$composite")
  done
fi

status=0
checked=0
for file in "${files[@]}"; do
  if [ ! -f "$file" ]; then
    echo "::error::$file does not exist"
    status=1
    continue
  fi
  line_number=0
  while IFS= read -r line || [ -n "$line" ]; do
    line_number=$((line_number + 1))
    if ! [[ "$line" =~ ^[[:space:]]*(-[[:space:]]+)?uses:[[:space:]]*(.*)$ ]]; then
      continue
    fi
    reference="${BASH_REMATCH[2]}"
    reference="${reference%%#*}"
    reference="${reference//\"/}"
    reference="${reference//\'/}"
    reference="${reference%"${reference##*[![:space:]]}"}"
    checked=$((checked + 1))
    case "$reference" in
      ./* | actions/*) continue ;;
    esac
    if ! [[ "$reference" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_./-]+@[0-9a-f]{40}$ ]]; then
      echo "::error file=${file#"$ROOT"/},line=$line_number::third-party action '$reference' must be pinned to a full commit SHA"
      status=1
    fi
  done < "$file"
done

if [ "$checked" -eq 0 ]; then
  echo "::error::no action reference found, the pin check scanned nothing"
  exit 1
fi
if [ "$status" -eq 0 ]; then
  echo "every third-party action in ${#files[@]} workflow(s) is pinned by SHA ($checked references checked)"
fi
exit "$status"
