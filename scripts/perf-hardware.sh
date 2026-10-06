#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/perf-hardware.sh --state <state> --label <label> [--frame-log <csv>] [--window <seconds>]

Measures the one Paneflow instance running on this machine for 60 s and writes
bench/results/hardware-<os>-<arch>-<session>-<state>-<label>-<stamp>.json:
CPU and resident memory of the desktop, host and worker, their work counters
(root_renders among them), GPU load from every source the machine offers, and
frame times from --frame-log. A source that cannot be read is recorded
not_measured with its reason, never 0. See "Real hardware protocol" in
bench/README.md.

States: idle-4-panes, agent-thinking, stream-4, panes-8.
Label:  the build under test, for example v0.17.5 or main-0a74eb30.
USAGE
}

cd "$(dirname "$0")/.."

state=""
label=""
frame_log=""
window=""
while [ $# -gt 0 ]; do
  case "$1" in
    --state) shift; state="${1:-}" ;;
    --label) shift; label="${1:-}" ;;
    --frame-log) shift; frame_log="${1:-}" ;;
    --window) shift; window="${1:-}" ;;
    --help | -h) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
  shift
done
case "$state" in
  idle-4-panes | agent-thinking | stream-4 | panes-8) ;;
  *) usage >&2; exit 2 ;;
esac
if ! printf '%s' "$label" | grep -Eq '^[A-Za-z0-9._-]+$'; then
  echo "--label is required and may hold only letters, digits, dot, dash and underscore" >&2
  exit 2
fi

uid=$(id -u)
desktops=()
workers=()
for pid in $(pgrep -u "$uid" -x paneflow || true); do
  case " $(ps -o args= -p "$pid" 2>/dev/null || true) " in
    *" serve run"*) workers+=("$pid") ;;
    *) desktops+=("$pid") ;;
  esac
done
read -r -a hosts <<<"$(pgrep -u "$uid" -x paneflow-host | tr '\n' ' ' || true)"
if [ "${#desktops[@]}" -ne 1 ]; then
  echo "expected exactly one running paneflow desktop, found ${#desktops[@]} (${desktops[*]:-none}): quit every other Paneflow instance and run this script from a terminal outside Paneflow" >&2
  exit 1
fi
if [ "${#hosts[@]}" -gt 1 ] || [ "${#workers[@]}" -gt 1 ]; then
  echo "more than one paneflow-host (${hosts[*]}) or worker (${workers[*]}) is running: stop the other instances' hosts and workers first" >&2
  exit 1
fi

version=""
exe=""
if [ -r "/proc/${desktops[0]}/exe" ]; then
  exe=$(readlink "/proc/${desktops[0]}/exe" || true)
else
  exe=$(ps -o comm= -p "${desktops[0]}" 2>/dev/null || true)
fi
if [ -n "$exe" ] && [ -x "$exe" ]; then
  if command -v timeout >/dev/null; then
    version=$(timeout 10 "$exe" --version 2>/dev/null | head -n 1 || true)
  else
    version=$("$exe" --version 2>/dev/null | head -n 1 || true)
  fi
fi

harness=$(cargo test --release --locked -p paneflow-host --test persistent_baseline --no-run --message-format=json \
  | grep -o '"executable":"[^"]*persistent_baseline-[^"]*"' | tail -n 1 | sed 's/^"executable":"//; s/"$//')
if [ ! -x "$harness" ]; then
  echo "the persistent_baseline harness was not built: $harness" >&2
  exit 2
fi

export PANEFLOW_HW_STATE="$state"
export PANEFLOW_HW_LABEL="$label"
export PANEFLOW_HW_VERSION="${version:-unknown ($exe)}"
export PANEFLOW_HW_DESKTOP_PID="${desktops[0]}"
export PANEFLOW_HW_HOST_PID="${hosts[0]:-}"
export PANEFLOW_HW_WORKER_PID="${workers[0]:-}"
if [ -n "$frame_log" ]; then
  export PANEFLOW_HW_FRAME_LOG="$frame_log"
fi
if [ -n "$window" ]; then
  export PANEFLOW_HW_WINDOW_S="$window"
fi
echo "measuring desktop ${desktops[0]}, host ${hosts[0]:-none}, worker ${workers[0]:-none} ($PANEFLOW_HW_VERSION) in state $state; leave the machine alone for the window"
"$harness" hardware_protocol --ignored --exact --nocapture --test-threads=1
