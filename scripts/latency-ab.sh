#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
root=$(pwd -P)

control_runs=9
cpu_runs=3
seed=$(date +%s)
build=1
while [ $# -gt 0 ]; do
  case "$1" in
    --control) control_runs=$2; shift 2 ;;
    --cpu) cpu_runs=$2; shift 2 ;;
    --seed) seed=$2; shift 2 ;;
    --no-build) build=0; shift ;;
    -h | --help)
      echo "usage: scripts/latency-ab.sh [--control N] [--cpu N] [--seed S] [--no-build]"
      echo "Runs the TD-17 A/B of terminal latency in a live debug window: N control runs (the host plays the video with its tab not displayed) and N cpu runs (the video displayed in the dock), interleaved by a seeded shuffle; writes rule.json before the first run and verdict.json after the last, under target/latency-ab/<stamp>/."
      exit 0
      ;;
    *) echo "unknown argument $1; see --help" >&2; exit 2 ;;
  esac
done
if [ "$cpu_runs" -lt 1 ] || [ $((control_runs % cpu_runs)) -ne 0 ]; then
  echo "--control ($control_runs) must be a multiple of --cpu ($cpu_runs), so every block holds the same number of control runs" >&2
  exit 2
fi

for variable in $(compgen -e | grep '^PANEFLOW_' || true); do
  unset "$variable"
done

if [ "$build" = 1 ]; then
  cargo build -p paneflow-app -p paneflow-host --locked
  cargo build --manifest-path crates/paneflow-browser-host/Cargo.toml --locked
fi
app="$root/target/debug/paneflow"
[ -x "$app" ] || { echo "no debug build at $app; run without --no-build" >&2; exit 2; }

stamp=$(date -u +%Y%m%dT%H%M%SZ)
out="$root/target/latency-ab/$stamp"
site="$out/site"
mkdir -p "$site"
cp "$root/scripts/latency-ab/video.html" "$site/video.html"
ln -s "$root/crates/paneflow-browser-host/tests/fixtures/clip.webm" "$site/clip.webm"

server=""
port=""
cleanup() {
  [ -n "$server" ] && kill "$server" 2>/dev/null || true
}
trap cleanup EXIT

start_server() {
  local log=$1
  port=""
  python3 -u -m http.server --bind 127.0.0.1 --directory "$site" 0 >"$log" 2>&1 &
  server=$!
  for _ in $(seq 1 50); do
    port=$(sed -n 's/.*port \([0-9]*\).*/\1/p' "$log" | head -1)
    [ -n "$port" ] && break
    sleep 0.1
  done
  [ -n "$port" ] || return 1
  python3 -c "import sys, urllib.request; urllib.request.urlopen(sys.argv[1], timeout=5).read()" "http://127.0.0.1:$port/video.html"
}

stop_server() {
  [ -n "$server" ] && kill "$server" 2>/dev/null || true
  [ -n "$server" ] && wait "$server" 2>/dev/null || true
  server=""
}

reap() {
  local home=$1 signal
  for signal in TERM KILL; do
    for _ in $(seq 1 50); do
      pgrep -f -- "$home" >/dev/null || return 0
      sleep 0.2
    done
    pkill "-$signal" -f -- "$home" 2>/dev/null || true
  done
  ! pgrep -f -- "$home" >/dev/null
}

session="${XDG_SESSION_TYPE:-unknown}"
display_server=x11
[ -n "${WAYLAND_DISPLAY:-}" ] && display_server=wayland
sha=$(git rev-parse --short=12 HEAD)
dirty=false
[ -n "$(git status --porcelain --untracked-files=no)" ] && dirty=true

python3 - "$out" "$control_runs" "$cpu_runs" "$seed" "$sha" "$dirty" "$display_server" "$session" <<'PY'
import json, random, sys, datetime, platform
out, control, cpu, seed, sha, dirty, display, session = sys.argv[1:]
control, cpu, seed = int(control), int(cpu), int(seed)
rng = random.Random(seed)
per_block = control // cpu
schedule = []
for block in range(cpu):
    arms = ["control"] * per_block + ["cpu"]
    rng.shuffle(arms)
    schedule.extend(arms)
entries = [{"file": f"run-{index + 1:02d}-{arm}.json", "arm": arm} for index, arm in enumerate(schedule)]
rule = {
    "registered_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "rule": "The CPU path regresses when any of its run medians exceeds the largest control run median, or any of its run 99th percentiles exceeds the largest control run 99th percentile. A run with an error, fewer than 300 samples, or a video that did not play invalidates the campaign, and so does a run whose median exceeds 1.5 times the median of its arm's run medians; an invalid campaign is run again in full, and no run is repeated or dropped.",
    "method": "Debug build, PANEFLOW_LATENCY_PROBE=1; the window types into its own terminal through Window::dispatch_keystroke: 5 warm-up z and backspace pairs, then 3 rounds of 104 letters at 45 ms and ctrl-u; one sample per keystroke from the terminal key handler to the end of its paint. Every run starts from a fresh home whose paneflow.json turns browser.display on, holding a copy of one usage ledger imported before the first run, so the worker's first-start import of the agents' history never runs during a measurement; a run during which it runs is invalid. Control: the host plays the video in a tab that is not displayed while the dock shows the Changes tab of a folder that is not a repository. CPU: the dock shows the video tab.",
    "outlier_factor": 1.5,
    "seed": seed,
    "commit": sha,
    "dirty": dirty == "true",
    "display_server": display,
    "session_type": session,
    "os": platform.platform(),
    "schedule": entries,
}
with open(f"{out}/rule.json", "w") as handle:
    json.dump(rule, handle, indent=2)
    handle.write("\n")
print(f"rule registered at {rule['registered_at']}: {' '.join(entry['arm'] for entry in entries)}")
PY

threads_of() {
  local home=$1 proc
  for proc in $(pgrep -f -- "$home"); do
    cat /proc/"$proc"/task/*/comm 2>/dev/null
  done
}

prepare_usage() {
  local home
  home=$(mktemp -d "$HOME/.paneflow-dev-latency-ab-XXXXXX")
  PANEFLOW_HOME="$home" "$app" serve start >/dev/null 2>&1 || true
  local waited=0
  while [ "$waited" -lt 3000 ]; do
    if [ -f "$home/usage/ledger.json" ] && ! threads_of "$home" | grep -q '^paneflow-usage-i'; then
      break
    fi
    sleep 0.2
    waited=$((waited + 1))
  done
  PANEFLOW_HOME="$home" "$app" serve stop >/dev/null 2>&1 || true
  PANEFLOW_HOME="$home" "$app" host stop >/dev/null 2>&1 || true
  reap "$home" || true
  if [ ! -f "$home/usage/ledger.json" ]; then
    rm -rf "$home"
    return 1
  fi
  cp -a "$home/usage" "$out/usage-template"
  rm -rf "$home"
}

run_one() {
  local file=$1 arm=$2
  local home work log pid url
  log="$out/${file%.json}.log"
  if ! start_server "$out/${file%.json}.server.log"; then
    echo "{\"arm\": \"$arm\", \"samples_ms\": [], \"keystrokes\": 0, \"error\": \"the fixture server did not answer before the run\"}" >"$out/$file"
    stop_server
    return
  fi
  url="http://127.0.0.1:$port/video.html"
  home=$(mktemp -d "$HOME/.paneflow-dev-latency-ab-XXXXXX")
  cp -a "$out/usage-template" "$home/usage"
  printf '{"browser": {"display": true}}\n' >"$home/paneflow.json"
  work=$(mktemp -d)
  (
    cd "$work"
    exec env PANEFLOW_HOME="$home" PANEFLOW_NO_TELEMETRY=1 PANEFLOW_LATENCY_PROBE=1 \
      PANEFLOW_LATENCY_SELF_TYPE=1 PANEFLOW_LATENCY_ARM="$arm" PANEFLOW_LATENCY_URL="$url" \
      PANEFLOW_LATENCY_OUT="$out/$file" RUST_LOG=warn "$app"
  ) >"$log" 2>&1 &
  pid=$!
  local waited=0
  while [ ! -f "$out/$file" ] && kill -0 "$pid" 2>/dev/null && [ "$waited" -lt 1800 ]; do
    sleep 0.1
    waited=$((waited + 1))
  done
  local imported=""
  threads_of "$home" | grep -q '^paneflow-usage-i' && imported="the usage import was running at the end of the run"
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  PANEFLOW_HOME="$home" "$app" serve stop >/dev/null 2>&1 || true
  PANEFLOW_HOME="$home" "$app" host stop >/dev/null 2>&1 || true
  stop_server
  local leaked=""
  reap "$home" || leaked="a process of $home survived SIGKILL"
  sleep 2
  if pgrep -f -- "$home" >/dev/null; then
    echo "$file: a process of the run started after it ended and was stopped: $(pgrep -af -- "$home" | tr '\n' ';')" >>"$out/cleanup.log"
    reap "$home" || leaked="a process of $home survived SIGKILL"
  fi
  rm -rf "$home" "$work"
  [ -f "$out/$file" ] || echo "{\"arm\": \"$arm\", \"samples_ms\": [], \"keystrokes\": 0, \"error\": \"the window never wrote its report; see ${log##*/}\"}" >"$out/$file"
  [ -n "$imported" ] && leaked="${leaked:+$leaked; }$imported"
  if [ -n "$leaked" ]; then
    python3 -c "import json,sys; p=sys.argv[1]; d=json.load(open(p)); d['error']=(d.get('error') or '') + sys.argv[2]; json.dump(d, open(p,'w'))" "$out/$file" "$leaked"
  fi
}

echo "$(date +%H:%M:%S) preparing the usage ledger once, so no run imports the agents' history"
prepare_usage || { echo "the usage import never finished in a fresh home" >&2; exit 1; }

python3 -c "import json,sys; [print(e['file'], e['arm']) for e in json.load(open(sys.argv[1]))['schedule']]" "$out/rule.json" |
  while read -r file arm; do
    echo "$(date +%H:%M:%S) $file"
    run_one "$file" "$arm" </dev/null
  done

python3 "$root/scripts/latency-ab/verdict.py" "$out"
echo "results in $out"
