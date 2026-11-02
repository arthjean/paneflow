#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
RELEASE_WORKFLOWS=(
  ".github/workflows/release.yml"
  ".github/workflows/audit.yml"
  ".github/workflows/repo_publish.yml"
  ".github/workflows/update_cask.yml"
)
XTRACE_RE='(^|[[:space:];&|(])(set|bash|sh)([[:space:]]+(-[A-Za-z]+|[+-]o[[:space:]]+[a-z]+))*[[:space:]]+(-[A-Za-z]*x[A-Za-z]*|-o[[:space:]]+xtrace)([[:space:]]|$)'

if [ "$#" -gt 0 ]; then
  pin_files=("$@")
  trace_files=("$@")
else
  pin_files=()
  for workflow in "${RELEASE_WORKFLOWS[@]}"; do
    pin_files+=("$ROOT/$workflow")
  done
  trace_files=("$ROOT"/.github/workflows/*.yml)
  for composite in "$ROOT"/.github/actions/*/action.yml; do
    pin_files+=("$composite")
    trace_files+=("$composite")
  done
fi

status=0

relative() {
  printf '%s' "${1#"$ROOT"/}"
}

checked=0
for file in "${pin_files[@]}"; do
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
      echo "::error file=$(relative "$file"),line=$line_number::$(relative "$file"):$line_number: third-party action '$reference' must be pinned to a full commit SHA"
      status=1
    fi
  done < "$file"
done

if [ "$checked" -eq 0 ]; then
  echo "::error::no action reference found, the pin check scanned nothing"
  exit 1
fi

traced=0
for file in "${trace_files[@]}"; do
  [ -f "$file" ] || continue
  line_number=0
  while IFS= read -r line || [ -n "$line" ]; do
    line_number=$((line_number + 1))
    line="${line%$'\r'}"
    if [[ "$line" =~ ^[[:space:]]*# ]]; then
      continue
    fi
    if [[ "$line" =~ $XTRACE_RE ]]; then
      echo "::error file=$(relative "$file"),line=$line_number::$(relative "$file"):$line_number: shell xtrace (-x, set -x, set -o xtrace) writes secrets into the public log; use bash -euo pipefail {0}"
      status=1
    fi
  done < "$file"
  traced=$((traced + 1))
done

if [ "$status" -eq 0 ]; then
  echo "every third-party action in ${#pin_files[@]} file(s) is pinned by SHA ($checked references checked)"
  echo "no shell xtrace in $traced workflow or composite action file(s)"
fi
exit "$status"
