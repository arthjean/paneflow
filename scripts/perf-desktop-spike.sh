#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
root=$(pwd)
display="${DISPLAY_SERVER:-xvfb}"
harness="${SPIKE_HARNESS:?set SPIKE_HARNESS to the persistent_baseline test executable}"
stamp=$(date -u +%Y%m%dT%H%M%SZ)
sha=$(git rev-parse --short=12 HEAD)
mkdir -p bench/results

export PANEFLOW_BENCH_OUT="$root/bench/results/desktop-spike-${display}-${stamp}-${sha}.json"
export PANEFLOW_BENCH_STAMP="$stamp"
export PANEFLOW_BENCH_SHA="$sha"
export PANEFLOW_BENCH_HOST="$root/target/release/paneflow-host"
export PANEFLOW_BENCH_FIXTURE="$root/target/release/paneflow-session-fixture"
export PANEFLOW_BENCH_CONTROLLER="$root/target/release/paneflow"
export PANEFLOW_BENCH_DESKTOP=1
export PANEFLOW_SPIKE_DISPLAY="$display"

run_harness() {
  (cd crates/paneflow-host && "$harness" desktop_headless_spike --ignored --exact --nocapture --test-threads=1)
}

case "$display" in
  xvfb)
    export -f run_harness
    export harness
    xvfb-run -a -s "-screen 0 1920x1080x24" bash -c run_harness 2>&1 | tee spike-xvfb.log
    ;;
  sway)
    runtime=$(mktemp -d)
    chmod 700 "$runtime"
    export XDG_RUNTIME_DIR="$runtime"
    WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 sway --unsupported-gpu >spike-sway-compositor.log 2>&1 &
    compositor=$!
    trap 'kill "$compositor" 2>/dev/null || true' EXIT
    for _ in $(seq 1 100); do
      socket=$(find "$runtime" -maxdepth 1 -name 'wayland-*' ! -name '*.lock' -print -quit)
      [ -n "$socket" ] && break
      sleep 0.1
    done
    if [ -z "${socket:-}" ]; then
      echo "sway headless did not create a Wayland socket" >&2
      cat spike-sway-compositor.log >&2
      exit 1
    fi
    export WAYLAND_DISPLAY
    WAYLAND_DISPLAY=$(basename "$socket")
    unset DISPLAY
    run_harness 2>&1 | tee spike-sway.log
    ;;
  *)
    echo "unknown display server: $display (expected xvfb or sway)" >&2
    exit 2
    ;;
esac
