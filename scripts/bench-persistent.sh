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
prebuilt=""
endurance=""
idle_minutes=""
while [ $# -gt 0 ]; do
  case "$1" in
    --endurance) shift; endurance="${1:-}" ;;
    --idle-minutes) shift; idle_minutes="${1:-}" ;;
    --set-baseline) mode="set-baseline" ;;
    --with-worker) worker=true ;;
    --with-desktop) worker=true; desktop=true ;;
    --no-followers) no_followers=true ;;
    --quick) quick=true ;;
    --seed-failure) seed_failure=true ;;
    --prior) shift; prior="${1:-}" ;;
    --worker-replacement) shift; replacement="${1:-}"; worker=true ;;
    --prebuilt) shift; prebuilt="${1:-}" ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

if [ -n "$prebuilt" ]; then
  prebuilt=$(cd "$prebuilt" && pwd -P)
  manifest="$prebuilt/candidate.json"
  if [ ! -f "$manifest" ]; then
    echo "no candidate manifest at $manifest" >&2
    exit 2
  fi
  full_sha=$(sed -n 's/^  "candidate_sha": "\([0-9a-f]*\)",$/\1/p' "$manifest")
  sha=${full_sha:0:12}
  dirty=$(sed -n 's/^  "dirty": \([a-z]*\),$/\1/p' "$manifest")
  if [ -z "$sha" ] || [ -z "$dirty" ]; then
    echo "unreadable candidate manifest: $manifest" >&2
    exit 2
  fi
  bin_dir="$prebuilt/bin"
  if [ -d "$prebuilt/PaneFlow.app/Contents/MacOS" ]; then
    bin_dir="$prebuilt/PaneFlow.app/Contents/MacOS"
  fi
  export PANEFLOW_BENCH_HOST="$bin_dir/paneflow-host"
  export PANEFLOW_BENCH_FIXTURE="$prebuilt/bin/paneflow-session-fixture"
  controller="$bin_dir/paneflow"
  harness="$prebuilt/bin/persistent_baseline"
  for required in "$PANEFLOW_BENCH_HOST" "$PANEFLOW_BENCH_FIXTURE" "$controller" "$harness"; do
    if [ ! -x "$required" ]; then
      echo "prebuilt package is missing $required" >&2
      exit 2
    fi
  done
else
  sha=$(git rev-parse --short=12 HEAD)
  dirty=false
  if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
    dirty=true
  fi
  unset PANEFLOW_BENCH_HOST PANEFLOW_BENCH_FIXTURE
  controller="$(pwd)/target/release/paneflow"
fi
stamp=$(date -u +%Y%m%dT%H%M%SZ)
mkdir -p bench/results
root=$(pwd)
suite="persistent"
test="persistent_session_baseline"
if [ -n "$endurance" ]; then
  suite="persistent-endurance"
  test="persistent_session_endurance"
  export PANEFLOW_BENCH_ENDURANCE_MINUTES="$endurance"
else
  unset PANEFLOW_BENCH_ENDURANCE_MINUTES
fi
if [ -n "$idle_minutes" ]; then
  export PANEFLOW_BENCH_IDLE_MINUTES="$idle_minutes"
else
  unset PANEFLOW_BENCH_IDLE_MINUTES
fi
out="$root/bench/results/${suite}-${stamp}-${sha}.json"

export PANEFLOW_BENCH_OUT="$out"
export PANEFLOW_BENCH_SHA="$sha"
export PANEFLOW_BENCH_DIRTY="$dirty"
export PANEFLOW_BENCH_STAMP="$stamp"
if [ -f bench/persistent-baseline.json ]; then
  export PANEFLOW_BENCH_BASELINE="$root/bench/persistent-baseline.json"
fi

if [ -z "$prebuilt" ]; then
  harness=$(cargo test --release --locked -p paneflow-host --test persistent_baseline --no-run --message-format=json \
    | grep -o '"executable":"[^"]*persistent_baseline-[^"]*"' | tail -n 1 | sed 's/^"executable":"//; s/"$//')
  cargo build --release --locked -p paneflow-app -p paneflow-host
  if [ ! -x "$harness" ]; then
    echo "the persistent_baseline harness was not built: $harness" >&2
    exit 2
  fi
  export PANEFLOW_BENCH_HOST="$root/target/release/paneflow-host"
  export PANEFLOW_BENCH_FIXTURE="$root/target/release/paneflow-session-fixture"
fi
if [ "$worker" = "true" ]; then
  export PANEFLOW_BENCH_CONTROLLER="$controller"
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
if [ -n "$prebuilt" ]; then
  "$harness" "$test" --ignored --exact --nocapture --test-threads=1
else
  (cd crates/paneflow-host && "$harness" "$test" --ignored --exact --nocapture --test-threads=1)
fi
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
if [ "$mode" = "set-baseline" ] && [ -z "$endurance" ]; then
  cp "$out" bench/persistent-baseline.json
  echo "baseline: bench/persistent-baseline.json now points at $sha"
fi
