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

  file="$(workflow timeout/unbounded.yml <<EOF
jobs:
  bounded:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - uses: actions/checkout@v5
  unbounded:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v5
        timeout-minutes: 5
EOF
)"
  expect "a job without timeout-minutes is named by file and line" 1 "unbounded\.yml:7: job 'unbounded' declares no timeout-minutes" "$check" "$file"

  file="$(workflow timeout/last-job.yml <<EOF
jobs:
  only:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v5
EOF
)"
  expect "the last job of a file is checked too" 1 "last-job\.yml:2: job 'only' declares no timeout-minutes" "$check" "$file"

  file="$(workflow timeout/bounded.yml <<EOF
jobs:
  build:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps: &steps
      - uses: actions/checkout@v5
  again:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps: *steps
  called:
    uses: ./.github/workflows/other.yml
EOF
)"
  expect "bounded jobs and a reusable workflow call pass" 0 "every job declares timeout-minutes \(3 job" "$check" "$file"
}

retry_cases() {
  local retry="$ROOT/scripts/ci-retry-network.sh"
  local fake="$WORK/fake-command.sh"
  cat > "$fake" <<'EOF'
#!/usr/bin/env bash
counter="$1"
failures="$2"
message="$3"
runs=0
[ -f "$counter" ] && runs="$(cat "$counter")"
runs=$((runs + 1))
echo "$runs" > "$counter"
if [ "$runs" -le "$failures" ]; then
  echo "$message"
  exit 7
fi
echo "command output"
EOF
  chmod +x "$fake"

  local signature index=0 counter listed
  listed="$("$retry" --list-transient)"
  while IFS= read -r signature; do
    index=$((index + 1))
    counter="$WORK/retry-count-$index"
    expect "the transient signature '$signature' is retried" 0 "succeeded on attempt 2 after a transient network failure" \
      env CI_RETRY_BACKOFF_SECONDS=0 "$retry" "$fake" "$counter" 1 "error: $signature while fetching"
    cases=$((cases + 1))
    if [ "$(cat "$counter")" != "2" ]; then
      echo "FAIL the transient signature '$signature' ran the command $(cat "$counter") time(s), expected 2"
      failures=$((failures + 1))
    fi
  done <<<"$listed"
  cases=$((cases + 1))
  if [ "$index" -lt 20 ]; then
    echo "FAIL --list-transient printed $index signature(s), expected at least 20"
    failures=$((failures + 1))
  fi
  for signature in ReadFailed ProtocolError; do
    cases=$((cases + 1))
    if ! grep -qFx "$signature" <<<"$listed"; then
      echo "FAIL the Zig signature $signature is not in the transient list"
      failures=$((failures + 1))
    fi
  done

  counter="$WORK/retry-count-real-error"
  expect "a failure with no transient signature is not retried and keeps its exit code" 7 "no transient network signature; not retrying" \
    env CI_RETRY_BACKOFF_SECONDS=0 "$retry" "$fake" "$counter" 1 "error[vulnerability]: a real advisory"
  cases=$((cases + 1))
  if [ "$(cat "$counter")" != "1" ]; then
    echo "FAIL a failure with no transient signature ran the command $(cat "$counter") time(s), expected 1"
    failures=$((failures + 1))
  fi

  counter="$WORK/retry-count-exhausted"
  expect "a transient failure that never clears fails after the last attempt" 7 "still failing on a transient network error after 3 attempts" \
    env CI_RETRY_BACKOFF_SECONDS=0 "$retry" "$fake" "$counter" 9 "503 Service Unavailable"
  cases=$((cases + 1))
  if [ "$(cat "$counter")" != "3" ]; then
    echo "FAIL a transient failure that never clears ran the command $(cat "$counter") time(s), expected 3"
    failures=$((failures + 1))
  fi

  counter="$WORK/retry-count-success"
  expect "a command that succeeds runs once" 0 "command output" \
    env CI_RETRY_BACKOFF_SECONDS=0 "$retry" "$fake" "$counter" 0 "unused"
}

helper_dir() {
  local dir="$WORK/helpers/$1"
  mkdir -p "$dir"
  head -c "$2" /dev/zero > "$dir/paneflow-shim"
  head -c "$3" /dev/zero > "$dir/paneflow-ai-hook"
  head -c "$4" /dev/zero > "$dir/paneflow-mcp"
  printf '%s' "$dir"
}

helper_size_cases() {
  local check="$ROOT/scripts/check-helper-sizes.sh"
  local dir

  dir="$(helper_dir within 400000 300000 400000)"
  expect "helpers under every cap pass" 0 "embedded helpers = 1100000 B" "$check" "$dir"

  dir="$(helper_dir hook-over 400000 384001 400000)"
  expect "a helper over its cap is named with its size and the cap" 1 "paneflow-ai-hook is 384001 B, over its hard cap of 384000 B" "$check" "$dir"

  dir="$(helper_dir at-caps 524288 384000 524288)"
  expect "helpers at their caps still fit the total cap" 0 "embedded helpers = 1432576 B" "$check" "$dir"

  dir="$(helper_dir near-cap 400000 383000 400000)"
  expect "a helper within 4 KiB of its cap warns and passes" 0 "::warning::paneflow-ai-hook is 383000 B, only 1000 B under its hard cap" "$check" "$dir"

  dir="$(helper_dir missing 400000 300000 400000)"
  rm "$dir/paneflow-mcp"
  expect "a missing helper fails instead of passing an unmeasured build" 1 "paneflow-mcp is missing" "$check" "$dir"

  dir="$(helper_dir stale-total 400000 300000 400000)"
  printf 'const OTHER: u64 = 1;\n' > "$WORK/build.rs"
  expect "an unreadable total cap fails" 1 "could not read EMBED_SIZE_LIMIT_BYTES" env PANEFLOW_BUILD_RS="$WORK/build.rs" "$check" "$dir"

  printf 'const EMBED_SIZE_LIMIT_BYTES: u64 = 1_000_000;\n' > "$WORK/build.rs"
  expect "helpers over the total cap of build.rs fail" 1 "the embedded helpers total 1100000 B, over the hard cap of 1000000 B" env PANEFLOW_BUILD_RS="$WORK/build.rs" "$check" "$dir"
}

check_workflows_cases
helper_size_cases
retry_cases

if [ "$failures" -ne 0 ]; then
  echo "::error::$failures of $cases CI script case(s) failed"
  exit 1
fi
echo "$cases CI script case(s) passed"
