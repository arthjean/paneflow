#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
root=$(pwd -P)

if [ $# -ne 2 ]; then
  echo "usage: scripts/perf-ab.sh <base-commit> <head-commit>" >&2
  exit 2
fi
base_ref=$1
head_ref=$2
rounds=10
cargo=${CARGO:-cargo}

for variable in $(compgen -e | grep '^PANEFLOW_' || true); do
  case "$variable" in
    PANEFLOW_PERF_AB_DIR) ;;
    *) unset "$variable" ;;
  esac
done

base_sha=$(git rev-parse --verify --quiet "$base_ref^{commit}") || {
  echo "base $base_ref is not a commit" >&2
  exit 2
}
head_sha=$(git rev-parse --verify --quiet "$head_ref^{commit}") || {
  echo "head $head_ref is not a commit" >&2
  exit 2
}
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  checkout_sha=$(git rev-parse HEAD)
  for side in base head; do
    sha_variable="${side}_sha"
    if [ "${!sha_variable}" = "$checkout_sha" ]; then
      echo "the working tree has uncommitted changes and $side resolves to its HEAD commit: the A/B measures commits, never a dirty tree; commit the changes first (git status --short)" >&2
      exit 2
    fi
  done
fi

out="${PANEFLOW_PERF_AB_DIR:-$root/target/perf-ab}"
mkdir -p "$out"
rm -rf "$out"/attempt-* "$out"/build-*.log "$out"/result.json "$out"/summary.md "$out"/verdict "$out"/attempt-verdict
scratch=$(mktemp -d "${TMPDIR:-/tmp}/paneflow-perf-ab.XXXXXX")
if [ -z "${CARGO_TARGET_DIR:-}" ]; then
  export CARGO_TARGET_DIR="$scratch/target"
fi

finish() {
  deliberate=true
  exit "$1"
}

cleanup() {
  local status=$?
  if [ "${deliberate:-false}" != true ] && [ "$status" -ne 0 ]; then
    echo "the A/B stopped on an unexpected error (exit $status)" >&2
    status=2
  fi
  for side in base head; do
    if [ -d "$scratch/$side" ]; then
      git worktree remove --force "$scratch/$side" >/dev/null 2>&1 || true
    fi
  done
  git worktree prune
  rm -rf "$scratch"
  exit "$status"
}
trap cleanup EXIT
trap 'finish 130' INT TERM

host_triple=$(rustc -vV | sed -n 's/^host: //p')
started=$(date +%s)

unavailable() {
  local side=$1 reason=$2 log=$3 code=3
  local sha_variable="${side}_sha"
  if [ "$side" = head ]; then
    code=4
  fi
  echo "$side ${!sha_variable:0:12} unavailable: $reason" >&2
  grep -E '^error' -A 6 "$log" | head -n 60 >&2 || true
  echo "log: $log" >&2
  finish "$code"
}

executable() {
  grep -o '"executable":"[^"]*/'"$2"'-[^"/]*"' "$1" | tail -n 1 | sed 's/^"executable":"//; s/"$//'
}

prepare() {
  local side=$1 sha=$2
  local worktree="$scratch/$side"
  git worktree add --detach --quiet "$worktree" "$sha"
  git ls-files --others --ignored --exclude-standard --directory -- native | while IFS= read -r ignored; do
    if [ -d "$root/$ignored" ]; then
      mkdir -p "$worktree/$ignored"
      cp -R "$root/$ignored." "$worktree/$ignored"
    else
      mkdir -p "$(dirname "$worktree/$ignored")"
      cp "$root/$ignored" "$worktree/$ignored"
    fi
  done
}

build() {
  local side=$1
  local worktree="$scratch/$side" bin="$scratch/bin-$side" log="$out/build-$side.log"
  mkdir -p "$bin"
  echo "== building $side $(git rev-parse --short=12 "$2")"
  set +e
  (
    set -e
    cd "$worktree"
    bash scripts/fetch-libghostty.sh --target "$host_triple"
    "$cargo" build --profile gates --locked -p paneflow-host
    "$cargo" test --profile gates --locked -p paneflow-app --bin paneflow --no-run \
      --message-format=json-render-diagnostics >"$bin/terminal.jsonl"
    if [ "$side" = head ]; then
      "$cargo" test --profile gates --locked -p paneflow-host --test persistent_baseline --no-run \
        --message-format=json-render-diagnostics >"$bin/harness.jsonl"
    fi
  ) >"$log" 2>&1
  local status=$?
  set -e
  if [ "$status" -ne 0 ]; then
    unavailable "$side" "the build failed" "$log"
  fi
  local terminal
  terminal=$(executable "$bin/terminal.jsonl" paneflow)
  if [ ! -x "$terminal" ]; then
    unavailable "$side" "the terminal suite executable was not built" "$log"
  fi
  cp "$terminal" "$bin/terminal-suite"
  cp "$CARGO_TARGET_DIR/gates/paneflow-host" "$bin/paneflow-host"
  if [ "$side" = head ]; then
    cp "$CARGO_TARGET_DIR/gates/paneflow-session-fixture" "$bin/paneflow-session-fixture"
    local harness
    harness=$(executable "$bin/harness.jsonl" persistent_baseline)
    if [ ! -x "$harness" ]; then
      unavailable head "the persistent_baseline harness was not built" "$log"
    fi
    cp "$harness" "$bin/harness"
  fi
}

run_slot() {
  local dir=$1 round=$2 slot=$3 side=$4
  local bin="$scratch/bin-$side" tag status
  tag=$(printf 'r%02d-%d-%s' "$round" "$slot" "$side")
  set +e
  (
    cd "$scratch/$side/src-app"
    PANEFLOW_BENCH_SKIP_IDLE=1 PANEFLOW_BENCH_SAMPLES_OUT="$dir/$tag-terminal.json" \
      "$bin/terminal-suite" terminal::perf_bench::terminal_pipeline_benchmark \
      --ignored --exact --nocapture --test-threads=1
  ) >"$dir/$tag-terminal.log" 2>&1
  status=$?
  set -e
  if [ "$status" -ne 0 ] || [ ! -s "$dir/$tag-terminal.json" ]; then
    unavailable "$side" "the terminal suite exited $status without samples in round $round; a commit older than the A/B sample export writes none" "$dir/$tag-terminal.log"
  fi
  set +e
  (
    cd "$scratch/head/crates/paneflow-host"
    PANEFLOW_BENCH_HOST="$bin/paneflow-host" \
      PANEFLOW_BENCH_FIXTURE="$scratch/bin-head/paneflow-session-fixture" \
      PANEFLOW_AB_SAMPLES_OUT="$dir/$tag-active.json" \
      "$scratch/bin-head/harness" ab::perf_ab_active_samples \
      --ignored --exact --nocapture --test-threads=1
  ) >"$dir/$tag-active.log" 2>&1
  status=$?
  set -e
  if [ "$status" -ne 0 ] || [ ! -s "$dir/$tag-active.json" ]; then
    unavailable "$side" "the active scenario exited $status without samples in round $round" "$dir/$tag-active.log"
  fi
}

measure() {
  local attempt=$1
  local dir="$out/attempt-$attempt"
  mkdir -p "$dir"
  for round in $(seq 1 "$rounds"); do
    echo "== attempt $attempt, round $round of $rounds"
    run_slot "$dir" "$round" 1 base
    run_slot "$dir" "$round" 2 head
    run_slot "$dir" "$round" 3 head
    run_slot "$dir" "$round" 4 base
  done
}

compare() {
  (
    cd "$scratch/head/crates/paneflow-host"
    PANEFLOW_AB_DIR="$out" \
      PANEFLOW_AB_BASE_REF="$base_ref" PANEFLOW_AB_BASE_SHA="$base_sha" \
      PANEFLOW_AB_HEAD_REF="$head_ref" PANEFLOW_AB_HEAD_SHA="$head_sha" \
      "$scratch/bin-head/harness" ab::perf_ab_compare --ignored --exact --nocapture --test-threads=1
  )
}

prepare base "$base_sha"
build base "$base_sha"
prepare head "$head_sha"
build head "$head_sha"

measure 1
compare
first=$(cat "$out/attempt-verdict")
if [ "$first" = regression ] || [ "$first" = uncalibrated ]; then
  echo "== the first execution was $first; measuring once more before any verdict"
  measure 2
  compare
fi

verdict=$(cat "$out/verdict")
echo "A/B finished in $(( $(date +%s) - started )) s: $verdict; report $out/result.json, summary $out/summary.md"
case "$verdict" in
  pass | unconfirmed_regression) finish 0 ;;
  regression) finish 1 ;;
  uncalibrated) finish 5 ;;
  *) finish 2 ;;
esac
