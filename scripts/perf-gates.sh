#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
root=$(pwd)

mode=gate
case "${1:-}" in
  "") ;;
  --refresh-alloc-baselines) mode=refresh ;;
  *)
    echo "usage: scripts/perf-gates.sh [--refresh-alloc-baselines]" >&2
    exit 2
    ;;
esac

for variable in $(compgen -e | grep '^PANEFLOW_' || true); do
  [ "$variable" = PANEFLOW_PERF_GATES_DIR ] || unset "$variable"
done

out="${PANEFLOW_PERF_GATES_DIR:-$root/target/perf-gates}"
mkdir -p "$out"
rm -f "$out"/*.json "$out"/*.log
started=$(date +%s)

export PANEFLOW_BENCH_STAMP
PANEFLOW_BENCH_STAMP=$(date -u +%Y%m%dT%H%M%SZ)
export PANEFLOW_BENCH_SHA
PANEFLOW_BENCH_SHA=$(git rev-parse --short=12 HEAD)
export PANEFLOW_BENCH_DIRTY=false
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  PANEFLOW_BENCH_DIRTY=true
fi

failed_steps=()
run_step() {
  local name=$1
  shift
  echo "== $name"
  set +e
  "$@" 2>&1 | tee "$out/$name.log"
  local status=${PIPESTATUS[0]}
  set -e
  if [ "$status" -ne 0 ]; then
    echo "::error::$name exited with status $status; see $out/$name.log"
    failed_steps+=("$name")
  fi
}

run_suite() {
  local suite=$1 test=$2
  PANEFLOW_BENCH_SKIP_IDLE=1 PANEFLOW_BENCH_OUT="$out/$suite.json" \
    cargo test --release --locked -p paneflow-app --bin paneflow "$test" \
    -- --ignored --exact --nocapture --test-threads=1
}

run_step terminal run_suite terminal terminal::perf_bench::terminal_pipeline_benchmark
run_step editor run_suite editor app::diff_dock::code::perf_bench::editor_pipeline_benchmark

if [ "$mode" = refresh ]; then
  for suite in terminal editor; do
    if [ ! -f "$out/$suite.json" ]; then
      echo "the $suite suite wrote no result; no baseline was refreshed" >&2
      exit 1
    fi
  done
  for suite in terminal editor; do
    cp "$out/$suite.json" "bench/$suite-alloc-baseline-linux.json"
    echo "baseline: bench/$suite-alloc-baseline-linux.json now points at $PANEFLOW_BENCH_SHA (dirty: $PANEFLOW_BENCH_DIRTY)"
  done
  exit 0
fi

run_step hook-burst env PANEFLOW_GATE_HOOK_BURST_OUT="$out/hook-burst.json" \
  cargo test --release --locked -p paneflow-host --lib \
  host::tests::a_hook_burst_with_slow_durability_loses_nothing_and_never_queues_behind_the_lock \
  -- --exact --nocapture

cargo build --release --locked -p paneflow-app -p paneflow-host
harness=$(cargo test --release --locked -p paneflow-host --test persistent_baseline --no-run --message-format=json \
  | grep -o '"executable":"[^"]*persistent_baseline-[^"]*"' | tail -n 1 | sed 's/^"executable":"//; s/"$//')
test -x "$harness"

display_number="${PANEFLOW_PERF_GATES_DISPLAY_NUMBER:-97}"
"${XVFB:-Xvfb}" ":$display_number" -screen 0 1920x1080x24 -nolisten tcp >"$out/xvfb.log" 2>&1 &
xvfb_pid=$!
trap 'kill "$xvfb_pid" 2>/dev/null || true' EXIT
for _ in $(seq 1 100); do
  [ -S "/tmp/.X11-unix/X$display_number" ] && break
  kill -0 "$xvfb_pid" 2>/dev/null || break
  sleep 0.1
done
if [ ! -S "/tmp/.X11-unix/X$display_number" ]; then
  echo "::error::the virtual display :$display_number did not start; the desktop budgets will fail as missing"
  cat "$out/xvfb.log" >&2 || true
fi
export DISPLAY=":$display_number"
unset WAYLAND_DISPLAY
for icd in /usr/share/vulkan/icd.d/lvp_icd.x86_64.json /usr/share/vulkan/icd.d/lvp_icd.json; do
  if [ -f "$icd" ]; then
    export VK_DRIVER_FILES="$icd" VK_ICD_FILENAMES="$icd"
    break
  fi
done
if [ -z "${VK_DRIVER_FILES:-}" ]; then
  echo "::warning::no Mesa lavapipe ICD found; the desktop uses whatever Vulkan adapter the machine has"
fi

run_step startup env PANEFLOW_BENCH_OUT="$out/startup.json" \
  cargo test --release --locked -p paneflow-app --bin paneflow \
  startup_bench::startup_first_frame_benchmark \
  -- --ignored --exact --nocapture --test-threads=1

run_step verifier bash -c 'cd crates/paneflow-host && "$1" gates:: gate_runs::' _ "$harness"

run_step gates env \
  PANEFLOW_PERF_GATES_DIR="$out" \
  PANEFLOW_BENCH_OUT="$out/perf-gates.json" \
  PANEFLOW_BENCH_HOST="$root/target/release/paneflow-host" \
  PANEFLOW_BENCH_FIXTURE="$root/target/release/paneflow-session-fixture" \
  PANEFLOW_BENCH_CONTROLLER="$root/target/release/paneflow" \
  bash -c 'cd crates/paneflow-host && "$1" perf_gates --ignored --exact --nocapture --test-threads=1' _ "$harness"

echo "performance gates finished in $(( $(date +%s) - started )) s; report: $out/perf-gates.json"
for step in verifier gates; do
  for failed in "${failed_steps[@]}"; do
    if [ "$failed" = "$step" ]; then
      exit 1
    fi
  done
done
