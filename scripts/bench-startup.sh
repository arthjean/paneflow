#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

mode="run"
if [ "${1:-}" = "--set-baseline" ]; then
  mode="set-baseline"
fi

sha=$(git rev-parse --short=12 HEAD)
dirty=false
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  dirty=true
fi
stamp=$(date -u +%Y%m%dT%H%M%SZ)
mkdir -p bench/results
root=$(pwd)
out="$root/bench/results/startup-${stamp}-${sha}.json"

export PANEFLOW_BENCH_OUT="$out"
export PANEFLOW_BENCH_SHA="$sha"
export PANEFLOW_BENCH_DIRTY="$dirty"
export PANEFLOW_BENCH_STAMP="$stamp"
if [ -f bench/startup-baseline.json ]; then
  export PANEFLOW_BENCH_BASELINE="$root/bench/startup-baseline.json"
fi

cargo build --release --locked -p paneflow-app

cargo test --release --locked -p paneflow-app --bin paneflow \
  startup_bench::startup_first_frame_benchmark \
  -- --ignored --exact --nocapture --test-threads=1

if [ ! -f "$out" ]; then
  echo "benchmark produced no result file: $out" >&2
  exit 1
fi
echo "result: $out"
if [ "$mode" = "set-baseline" ]; then
  cp "$out" bench/startup-baseline.json
  echo "baseline: bench/startup-baseline.json now points at $sha"
fi
