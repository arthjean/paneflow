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
out="$root/bench/results/${stamp}-${sha}.json"

export PANEFLOW_BENCH_OUT="$out"
export PANEFLOW_BENCH_SHA="$sha"
export PANEFLOW_BENCH_DIRTY="$dirty"
export PANEFLOW_BENCH_STAMP="$stamp"
if [ -f bench/baseline.json ]; then
  export PANEFLOW_BENCH_BASELINE="$root/bench/baseline.json"
fi

cargo test --release --locked -p paneflow-app --bin paneflow \
  terminal::perf_bench::terminal_pipeline_benchmark \
  -- --ignored --exact --nocapture --test-threads=1

if [ ! -f "$out" ]; then
  echo "benchmark produced no result file: $out" >&2
  exit 1
fi
echo "result: $out"
if [ "$mode" = "set-baseline" ]; then
  cpu_share=$(sed -n 's/^[[:space:]]*"cpu_share":[[:space:]]*\([0-9.eE+-]*\).*/\1/p' "$out" | head -n 1)
  if [ -z "$cpu_share" ]; then
    echo "the result carries no cpu_share, refusing to record a baseline from it: $out" >&2
    exit 1
  fi
  if awk "BEGIN { exit !($cpu_share < 0.9) }"; then
    echo "cpu_share $cpu_share is below 0.90: this run got less than 90% of a core, so its timings are inflated and every later comparison against them would read as a false improvement. Close the competing workload and run again." >&2
    exit 1
  fi
  cp "$out" bench/baseline.json
  echo "baseline: bench/baseline.json now points at $sha"
fi
