#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/bench-editor.sh [--set-baseline] [--help]

Runs the paneflow-editor-bench suite under the release profile and writes
bench/results/editor-<stamp>-<sha>.json, then prints a Markdown table between
the PANEFLOW_BENCH_TABLE_BEGIN and PANEFLOW_BENCH_TABLE_END markers.

Options:
  --set-baseline  Copy the fresh result over bench/editor-baseline.json.
  --help          Print this message and exit.

Environment:
  PANEFLOW_BENCH_OUT         Result file the suite writes. Set by this script.
  PANEFLOW_BENCH_BASELINE    Baseline the table compares against. Set by this
                             script when bench/editor-baseline.json exists;
                             without it the table drops its comparison columns.
  PANEFLOW_BENCH_SHA         Short commit the result records. Set by this script.
  PANEFLOW_BENCH_DIRTY       Whether the tracked worktree is dirty.
  PANEFLOW_BENCH_STAMP       UTC stamp the result records.
  PANEFLOW_BENCH_ALLOW_DEBUG Allow a debug-profile run, which the suite refuses.
  PANEFLOW_BENCH_SKIP_SHAPE  Skip the platform shaping probe.
USAGE
}

cd "$(dirname "$0")/.."

mode="run"
case "${1:-}" in
  --help | -h)
    usage
    exit 0
    ;;
  --set-baseline)
    mode="set-baseline"
    ;;
  "") ;;
  *)
    usage >&2
    exit 2
    ;;
esac

sha=$(git rev-parse --short=12 HEAD)
dirty=false
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  dirty=true
fi
stamp=$(date -u +%Y%m%dT%H%M%SZ)
mkdir -p bench/results
root=$(pwd)
out="$root/bench/results/editor-${stamp}-${sha}.json"

export PANEFLOW_BENCH_OUT="$out"
export PANEFLOW_BENCH_SHA="$sha"
export PANEFLOW_BENCH_DIRTY="$dirty"
export PANEFLOW_BENCH_STAMP="$stamp"
if [ -f bench/editor-baseline.json ]; then
  export PANEFLOW_BENCH_BASELINE="$root/bench/editor-baseline.json"
else
  unset PANEFLOW_BENCH_BASELINE
fi

cargo test --release --locked -p paneflow-app --bin paneflow \
  app::diff_dock::code::perf_bench::editor_pipeline_benchmark \
  -- --ignored --exact --nocapture --test-threads=1

if [ ! -f "$out" ]; then
  echo "benchmark produced no result file: $out" >&2
  exit 1
fi
echo "result: $out"
if [ "$mode" = "set-baseline" ]; then
  cp "$out" bench/editor-baseline.json
  echo "baseline: bench/editor-baseline.json now points at $sha"
fi
