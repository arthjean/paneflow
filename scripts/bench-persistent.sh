#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

mode="run"
worker=false
desktop=false
no_followers=false
quick=false
seed_failure=false
prior=""
replacement=""
while [ $# -gt 0 ]; do
  case "$1" in
    --set-baseline) mode="set-baseline" ;;
    --with-worker) worker=true ;;
    --with-desktop) worker=true; desktop=true ;;
    --no-followers) no_followers=true ;;
    --quick) quick=true ;;
    --seed-failure) seed_failure=true ;;
    --prior) shift; prior="${1:-}" ;;
    --worker-replacement) shift; replacement="${1:-}"; worker=true ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

sha=$(git rev-parse --short=12 HEAD)
dirty=false
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  dirty=true
fi
stamp=$(date -u +%Y%m%dT%H%M%SZ)
mkdir -p bench/results
root=$(pwd)
out="$root/bench/results/persistent-${stamp}-${sha}.json"

export PANEFLOW_BENCH_OUT="$out"
export PANEFLOW_BENCH_SHA="$sha"
export PANEFLOW_BENCH_DIRTY="$dirty"
export PANEFLOW_BENCH_STAMP="$stamp"
if [ -f bench/persistent-baseline.json ]; then
  export PANEFLOW_BENCH_BASELINE="$root/bench/persistent-baseline.json"
fi

cargo build --release --locked -p paneflow-host
if [ "$worker" = "true" ]; then
  cargo build --release --locked -p paneflow-app
  export PANEFLOW_BENCH_CONTROLLER="$root/target/release/paneflow"
else
  unset PANEFLOW_BENCH_CONTROLLER
fi
if [ "$desktop" = "true" ]; then
  export PANEFLOW_BENCH_DESKTOP=1
else
  unset PANEFLOW_BENCH_DESKTOP
fi
if [ "$no_followers" = "true" ]; then
  export PANEFLOW_BENCH_NO_FOLLOWERS=1
else
  unset PANEFLOW_BENCH_NO_FOLLOWERS
fi
if [ "$quick" = "true" ]; then
  export PANEFLOW_BENCH_QUICK=1
else
  unset PANEFLOW_BENCH_QUICK
fi
if [ "$seed_failure" = "true" ]; then
  export PANEFLOW_BENCH_SEED_FAILURE=1
else
  unset PANEFLOW_BENCH_SEED_FAILURE
fi
if [ -n "$prior" ]; then
  export PANEFLOW_BENCH_PRIOR_RESULT="$prior"
else
  unset PANEFLOW_BENCH_PRIOR_RESULT
fi
if [ -n "$replacement" ]; then
  export PANEFLOW_BENCH_CONTROLLER_REPLACEMENT="$replacement"
else
  unset PANEFLOW_BENCH_CONTROLLER_REPLACEMENT
fi

set +e
cargo test --release --locked -p paneflow-host --test persistent_baseline \
  persistent_session_baseline \
  -- --ignored --exact --nocapture --test-threads=1
status=$?
set -e

if [ ! -f "$out" ]; then
  echo "benchmark produced no result file: $out" >&2
  exit 1
fi
echo "result: $out"
if [ "$status" -ne 0 ]; then
  echo "persistent-path thresholds failed (exit $status); the artifact above is retained and a rerun must pass --prior $out" >&2
  exit "$status"
fi
if [ "$mode" = "set-baseline" ]; then
  cp "$out" bench/persistent-baseline.json
  echo "baseline: bench/persistent-baseline.json now points at $sha"
fi
