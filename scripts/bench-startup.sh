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
if [ "$mode" = "set-baseline" ] && [ "$dirty" = "true" ]; then
  echo "the tracked worktree is dirty: commit before recording a baseline, so it records a commit that exists. Check with: git status --porcelain --untracked-files=no" >&2
  exit 1
fi
stamp=$(date -u +%Y%m%dT%H%M%SZ)
mkdir -p bench/results
root=$(pwd)
out="$root/bench/results/startup-${stamp}-${sha}.json"

export PANEFLOW_BENCH_OUT="$out"
export PANEFLOW_BENCH_SHA="$sha"
export PANEFLOW_BENCH_DIRTY="$dirty"
export PANEFLOW_BENCH_STAMP="$stamp"
export PANEFLOW_BENCH_BASELINE_DIR="$root/bench/baselines"

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
  platform=$(sed -n 's/^  "platform": "\([^"]*\)",\{0,1\}$/\1/p' "$out" | head -n 1)
  if [ -z "$platform" ]; then
    echo "the result names no platform, refusing to record a baseline from it: $out" >&2
    exit 1
  fi
  mkdir -p "bench/baselines/$platform"
  cp "$out" "bench/baselines/$platform/startup.json"
  echo "baseline: bench/baselines/$platform/startup.json now points at $sha"
fi
