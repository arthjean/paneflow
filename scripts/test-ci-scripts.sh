#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PINNED_CACHE="Swatinem/rust-cache@42dc69e1aa15d09112580998cf2ef0119e2e91ae"
cases=0
failures=0

expect() {
  local name="$1" expected_status="$2" pattern="$3"
  shift 3
  local output status=0
  output="$("$@" 2>&1)" || status=$?
  cases=$((cases + 1))
  if [ "$status" -ne "$expected_status" ]; then
    echo "FAIL $name: exit $status, expected $expected_status"
    printf '%s\n' "$output" | sed 's/^/    /'
    failures=$((failures + 1))
    return
  fi
  if [ -n "$pattern" ] && ! grep -qE -- "$pattern" <<<"$output"; then
    echo "FAIL $name: output does not match /$pattern/"
    printf '%s\n' "$output" | sed 's/^/    /'
    failures=$((failures + 1))
    return
  fi
  echo "ok   $name"
}

workflow() {
  local path="$WORK/$1"
  mkdir -p "$(dirname "$path")"
  cat > "$path"
  printf '%s' "$path"
}

check_workflows_cases() {
  local check="$ROOT/scripts/check-workflows.sh"
  local file

  file="$(workflow cache/saves-on-main.yml <<EOF
jobs:
  build:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - uses: $PINNED_CACHE
        with:
          key: build
          save-if: \${{ github.ref == 'refs/heads/main' }}
EOF
)"
  expect "a rust-cache step that saves from main only passes" 0 "saves from main only \(1 step" "$check" "$file"

  file="$(workflow cache/no-save-if.yml <<EOF
jobs:
  build:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - uses: actions/checkout@v5
      - uses: $PINNED_CACHE
        with:
          key: build
      - run: cargo build --locked
EOF
)"
  expect "a rust-cache step without save-if is named by file and line" 1 "no-save-if\.yml:7: Swatinem/rust-cache must declare save-if" "$check" "$file"

  file="$(workflow cache/named-step.yml <<EOF
jobs:
  build:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - name: Cache
        uses: $PINNED_CACHE
      - name: Unrelated step
        with:
          save-if: \${{ github.ref == 'refs/heads/main' }}
EOF
)"
  expect "a save-if on the next step does not cover the cache step" 1 "named-step\.yml:7: Swatinem/rust-cache must declare save-if" "$check" "$file"

  file="$(workflow cache/saves-everywhere.yml <<EOF
jobs:
  build:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - uses: $PINNED_CACHE
        with:
          save-if: \${{ github.event_name != 'pull_request' }}
EOF
)"
  expect "a save-if that also saves from a tag is refused" 1 "saves-everywhere\.yml:6: Swatinem/rust-cache must declare save-if" "$check" "$file"

  file="$(workflow release-cached/release.yml <<EOF
cache-mode: none
jobs:
  build:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - uses: $PINNED_CACHE
        with:
          save-if: \${{ github.ref == 'refs/heads/main' }}
EOF
)"
  expect "a rust-cache step in the release workflow is refused" 1 "release\.yml:7: the release workflow builds without a cache" "$check" "$file"

  file="$(workflow release-open/release.yml <<EOF
jobs:
  build:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - uses: actions/checkout@v5
EOF
)"
  expect "a release workflow without cache-mode: none is refused" 1 "release\.yml:1: the release workflow must declare cache-mode: none" "$check" "$file"
}

check_workflows_cases

if [ "$failures" -ne 0 ]; then
  echo "::error::$failures of $cases CI script case(s) failed"
  exit 1
fi
echo "$cases CI script case(s) passed"
