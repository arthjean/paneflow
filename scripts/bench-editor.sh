#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/bench-editor.sh [--set-baseline] [--help]

Runs the paneflow-editor-bench suite under the release profile and writes
bench/results/editor-<stamp>-<sha>.json, then prints a Markdown table between
the PANEFLOW_BENCH_TABLE_BEGIN and PANEFLOW_BENCH_TABLE_END markers.

Options:
  --set-baseline  Copy the fresh result over
                  bench/baselines/<os>-<arch>/editor.json. Refused from a dirty
                  tracked worktree, and when the run reports a cpu_share below
                  0.90, because a contended run inflates every timing it would
                  freeze.
  --help          Print this message and exit.

Environment:
  PANEFLOW_BENCH_OUT         Result file the suite writes. Set by this script.
  PANEFLOW_BENCH_BASELINE_DIR Directory holding one baseline directory per
                             <os>-<arch>. Set by this script to bench/baselines;
                             the suite compares only against the baseline of
                             its own platform and otherwise drops the
                             comparison columns.
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
if [ "$mode" = "set-baseline" ] && [ "$dirty" = "true" ]; then
  echo "the tracked worktree is dirty: commit before recording a baseline, so it records a commit that exists. Check with: git status --porcelain --untracked-files=no" >&2
  exit 1
fi
stamp=$(date -u +%Y%m%dT%H%M%SZ)
mkdir -p bench/results
root=$(pwd)
out="$root/bench/results/editor-${stamp}-${sha}.json"

export PANEFLOW_BENCH_OUT="$out"
export PANEFLOW_BENCH_SHA="$sha"
export PANEFLOW_BENCH_DIRTY="$dirty"
export PANEFLOW_BENCH_STAMP="$stamp"
export PANEFLOW_BENCH_BASELINE_DIR="$root/bench/baselines"

cargo test --release --locked -p paneflow-app --bin paneflow \
  app::diff_dock::code::perf_bench::editor_pipeline_benchmark \
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
  platform=$(sed -n 's/^  "platform": "\([^"]*\)",\{0,1\}$/\1/p' "$out" | head -n 1)
  if [ -z "$platform" ]; then
    echo "the result names no platform, refusing to record a baseline from it: $out" >&2
    exit 1
  fi
  mkdir -p "bench/baselines/$platform"
  cp "$out" "bench/baselines/$platform/editor.json"
  echo "baseline: bench/baselines/$platform/editor.json now points at $sha"
fi
