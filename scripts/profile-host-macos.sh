#!/usr/bin/env bash
set -uo pipefail

if [ $# -ne 5 ]; then
  echo "usage: $0 <paneflow-host> <paneflow-session-fixture> <memory|cpu> <out dir> <label>" >&2
  exit 2
fi
host_bin="$1"
fixture="$2"
mode="$3"
outdir="$4"
label="$5"
case "$mode" in
  memory|cpu) ;;
  *) echo "mode must be memory or cpu" >&2; exit 2 ;;
esac
if [ "$(uname -s)" != "Darwin" ]; then
  echo "this profile driver runs on macOS" >&2
  exit 2
fi
mkdir -p "$outdir"
outdir=$(cd "$outdir" && pwd -P)
host_bin=$(cd "$(dirname "$host_bin")" && pwd -P)/$(basename "$host_bin")
fixture=$(cd "$(dirname "$fixture")" && pwd -P)/$(basename "$fixture")
work=$(mktemp -d "${TMPDIR:-/tmp}/pfp.XXXXXX")
preexisting=" $(pgrep -f "$fixture" | tr '\n' ' ')"
mkdir -p "$work/home"
export PANEFLOW_HOME="$work/home"

sample_phase() {
  local pid="$1" phase="$2" rss footprint threads fds
  rss=$(ps -o rss= -p "$pid" | tr -d ' ')
  vmmap --summary "$pid" >"$outdir/vmmap-$label-$phase.txt" 2>&1
  footprint=$(sed -n 's/^Physical footprint: *//p' "$outdir/vmmap-$label-$phase.txt" | head -n 1)
  threads=$(($(ps -M -p "$pid" | wc -l) - 1))
  fds=$(lsof -n -P -p "$pid" 2>/dev/null | awk 'NR > 1 && $4 ~ /^[0-9]+/' | wc -l | tr -d ' ')
  printf '%s\t%s\trss_kib=%s\tphys_footprint=%s\tthreads=%s\tfds=%s\n' "$label" "$phase" "$rss" "$footprint" "$threads" "$fds" | tee -a "$outdir/memory.tsv"
}

serving() {
  "$host_bin" session list >/dev/null 2>&1
}

create() {
  "$host_bin" session create --cwd "$work" --shell "$fixture" -- "$@" | sed -n 's/.*"session": "\([^"]*\)".*/\1/p' | head -n 1
}

wait_exited() {
  local i
  for i in $(seq 1 600); do
    "$host_bin" session inspect "$1" 2>/dev/null | grep -q '"state": "exited"' && return 0
    sleep 0.1
  done
  return 1
}

"$host_bin" serve >"$outdir/host-$label.log" 2>&1 </dev/null &
pid=$!
for _ in $(seq 1 100); do
  serving && break
  sleep 0.1
done
serving || { echo "host did not serve" >&2; kill -TERM "$pid"; exit 1; }
sample_phase "$pid" empty
idle=()
for _ in $(seq 1 50); do
  idle+=("$(create idle)")
done
sleep 10
sample_phase "$pid" idle50
if [ "$mode" = "cpu" ]; then
  sample "$pid" 30 -file "$outdir/sample-idle-$label.txt" >/dev/null 2>&1
  streams=()
  for _ in $(seq 1 10); do
    streams+=("$(create stream 1048576 45)")
  done
  sleep 3
  sample "$pid" 30 -file "$outdir/sample-output-$label.txt" >/dev/null 2>&1
  for s in "${streams[@]}"; do
    wait_exited "$s"
  done
else
  for _ in $(seq 1 10); do
    wait_exited "$(create flood 1048576)"
  done
fi
sample_phase "$pid" after_output
for _ in $(seq 1 5); do
  churn=()
  for _ in $(seq 1 10); do
    churn+=("$(create flood 65536)")
  done
  for s in "${churn[@]}"; do
    wait_exited "$s"
    "$host_bin" session remove "$s" >/dev/null 2>&1
  done
done
sleep 6
sample_phase "$pid" after_churn
for s in "${idle[@]}"; do
  "$host_bin" session stop "$s" >/dev/null 2>&1
done
sleep 6
sample_phase "$pid" after_stop
kill -TERM "$pid"
wait "$pid" 2>/dev/null
survivors=""
for survivor in $(pgrep -f "$fixture"); do
  case "$preexisting " in
    *" $survivor "*) ;;
    *) survivors="$survivors$survivor " ;;
  esac
done
printf '%s\tsurvivors\t%s\n' "$label" "${survivors:-none}" | tee -a "$outdir/memory.tsv"
rm -rf "$work"
