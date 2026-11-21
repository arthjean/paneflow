#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/bench-worker.sh [--help]

Runs the paneflow-worker-bench suite under the gates profile and writes
bench/results/worker-<stamp>-<sha>.json: the median, 95th percentile and
maximum of 1,000 no-op requests to a worker over its local endpoint, and the
median and maximum time the endpoint stops answering across 20 worker
replacements triggered as an update triggers them. A run whose two halves
differ by more than 10 % is discarded and repeated; the result lists the
discarded attempts. No budget is gated.

Environment:
  PANEFLOW_BENCH_OUT    Result file the suite writes. Set by this script.
  PANEFLOW_BENCH_SHA    Short commit the result records. Set by this script.
  PANEFLOW_BENCH_DIRTY  Whether the tracked worktree is dirty.
  PANEFLOW_BENCH_STAMP  UTC stamp the result records.
USAGE
}

cd "$(dirname "$0")/.."

case "${1:-}" in
  --help | -h)
    usage
    exit 0
    ;;
  "") ;;
  *)
    usage >&2
    exit 2
    ;;
esac

for variable in $(compgen -e | grep '^PANEFLOW_' || true); do
  unset "$variable"
done

sha=$(git rev-parse --short=12 HEAD)
dirty=false
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  dirty=true
fi
stamp=$(date -u +%Y%m%dT%H%M%SZ)
mkdir -p bench/results
root=$(pwd)
out="$root/bench/results/worker-${stamp}-${sha}.json"

export PANEFLOW_BENCH_OUT="$out"
export PANEFLOW_BENCH_SHA="$sha"
export PANEFLOW_BENCH_DIRTY="$dirty"
export PANEFLOW_BENCH_STAMP="$stamp"

cargo test --profile gates --locked -p paneflow-app --test worker_round_trip \
  worker_round_trip_benchmark -- --ignored --exact --nocapture --test-threads=1

if [ ! -f "$out" ]; then
  echo "benchmark produced no result file: $out" >&2
  exit 1
fi
echo "result: $out"
