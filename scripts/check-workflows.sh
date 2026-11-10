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
GHOSTTY_PIN_RE='^[[:space:]]*-?[[:space:]]*GHOSTTY_SHA:[[:space:]]*(.*)$'
RUST_CACHE_RE='^([[:space:]]*)(-[[:space:]]+)?uses:[[:space:]]*Swatinem/rust-cache@'
SAVE_ON_MAIN="save-if:\${{github.ref=='refs/heads/main'}}"

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
    if [[ "$line" =~ $GHOSTTY_PIN_RE ]] && [[ "${BASH_REMATCH[1]}" != '${{'* ]]; then
      echo "::error file=$(relative "$file"),line=$line_number::$(relative "$file"):$line_number: literal GHOSTTY_SHA pin; read source_sha from native/libghostty/manifest.toml through .github/actions/libghostty-manifest"
      status=1
    fi
  done < "$file"
  traced=$((traced + 1))
done

cache_steps=0
jobs_checked=0
for file in "${trace_files[@]}"; do
  [ -f "$file" ] || continue
  mapfile -t lines < "$file"
  total=${#lines[@]}
  is_release=0
  cache_mode_none=0
  if [ "$(basename "$file")" = "release.yml" ]; then
    is_release=1
  fi
  for ((i = 0; i < total; i++)); do
    line="${lines[i]%$'\r'}"
    if [[ "$line" =~ ^cache-mode:[[:space:]]*none[[:space:]]*$ ]]; then
      cache_mode_none=1
    fi
    if ! [[ "$line" =~ $RUST_CACHE_RE ]]; then
      continue
    fi
    step_indent=${#BASH_REMATCH[1]}
    if [ -z "${BASH_REMATCH[2]}" ]; then
      step_indent=$((step_indent - 2))
    fi
    cache_steps=$((cache_steps + 1))
    if ((is_release == 1)); then
      echo "::error file=$(relative "$file"),line=$((i + 1))::$(relative "$file"):$((i + 1)): the release workflow builds without a cache; remove this Swatinem/rust-cache step"
      status=1
      continue
    fi
    saves_on_main=0
    for ((j = i + 1; j < total; j++)); do
      next="${lines[j]%$'\r'}"
      if [[ "$next" =~ ^[[:space:]]*(#.*)?$ ]]; then
        continue
      fi
      [[ "$next" =~ ^([[:space:]]*) ]]
      if ((${#BASH_REMATCH[1]} <= step_indent)); then
        break
      fi
      if [ "${next//[[:space:]]/}" = "$SAVE_ON_MAIN" ]; then
        saves_on_main=1
        break
      fi
    done
    if ((saves_on_main == 0)); then
      echo "::error file=$(relative "$file"),line=$((i + 1))::$(relative "$file"):$((i + 1)): Swatinem/rust-cache must declare save-if: \${{ github.ref == 'refs/heads/main' }} so that only main writes the cache"
      status=1
    fi
  done
  if ((is_release == 1 && cache_mode_none == 0)); then
    echo "::error file=$(relative "$file"),line=1::$(relative "$file"):1: the release workflow must declare cache-mode: none at the top level"
    status=1
  fi

  in_jobs=0
  job=""
  job_line=0
  job_bounded=0
  close_job() {
    if [ -n "$job" ] && ((job_bounded == 0)); then
      echo "::error file=$(relative "$file"),line=$job_line::$(relative "$file"):$job_line: job '$job' declares no timeout-minutes; set at most 1.5 times its longest observed run, or 60 for a job with no history"
      status=1
    fi
    if [ -n "$job" ]; then
      jobs_checked=$((jobs_checked + 1))
    fi
    job=""
  }
  for ((i = 0; i < total; i++)); do
    line="${lines[i]%$'\r'}"
    if [[ "$line" =~ ^jobs:[[:space:]]*(#.*)?$ ]]; then
      in_jobs=1
      continue
    fi
    if ((in_jobs == 0)); then
      continue
    fi
    if [[ "$line" =~ ^[^[:space:]#] ]]; then
      close_job
      in_jobs=0
      continue
    fi
    if [[ "$line" =~ ^\ \ ([A-Za-z0-9_-]+):(.*)$ ]]; then
      close_job
      job="${BASH_REMATCH[1]}"
      job_line=$((i + 1))
      job_bounded=0
      if [[ "${BASH_REMATCH[2]}" =~ ^[[:space:]]*\* ]]; then
        job_bounded=1
      fi
      continue
    fi
    if [[ "$line" =~ ^\ \ \ \ timeout-minutes:[[:space:]]*[^[:space:]#] ]] || [[ "$line" =~ ^\ \ \ \ uses:[[:space:]] ]]; then
      job_bounded=1
    fi
  done
  close_job
done

if [ "$status" -eq 0 ]; then
  echo "every third-party action in ${#pin_files[@]} file(s) is pinned by SHA ($checked references checked)"
  echo "no shell xtrace and no literal GHOSTTY_SHA pin in $traced workflow or composite action file(s)"
  echo "every Swatinem/rust-cache step saves from main only ($cache_steps step(s)), and the release workflow uses no cache"
  echo "every job declares timeout-minutes ($jobs_checked job(s))"
fi
exit "$status"
