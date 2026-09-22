#!/usr/bin/env bash
set -uo pipefail

app=""
fixture=""
out=""
route="unspecified"
while [ $# -gt 0 ]; do
  case "$1" in
    --app) shift; app="${1:-}" ;;
    --fixture) shift; fixture="${1:-}" ;;
    --out) shift; out="${1:-}" ;;
    --route) shift; route="${1:-}" ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done
if [ ! -x "$app/bin/paneflow-host" ] || [ ! -x "$fixture" ] || [ -z "$out" ]; then
  echo "usage: $0 --app <extracted paneflow.app> --fixture <paneflow-session-fixture> --out <ledger.tsv> [--route <install route>]" >&2
  exit 2
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/pfq.XXXXXX")
mkdir -p "$work/bin" "$work/home" "$work/run"
chmod 700 "$work/run"
cp -R "$app" "$work/app"
cp "$fixture" "$work/bin/paneflow-session-fixture"
host_bin="$work/app/bin/paneflow-host"
desktop_bin="$work/app/bin/paneflow"
fixture_bin="$work/bin/paneflow-session-fixture"
host_real=$(readlink -f "$host_bin")
fixture_real=$(readlink -f "$fixture_bin")
export PANEFLOW_HOME="$work/home"
export XDG_RUNTIME_DIR="$work/run"
sessions_dir="$PANEFLOW_HOME/host/sessions"
evidence="$(dirname "$out")"
mkdir -p "$evidence"
: > "$out"
failures=0

record() {
  printf '%s\t%s\t%s\n' "$1" "$2" "$3" | tee -a "$out"
  if [ "$2" = "fail" ]; then
    failures=$((failures + 1))
  fi
}

pids_of() {
  local target="$1" dir
  for dir in /proc/[0-9]*; do
    if [ "$(readlink "$dir/exe" 2>/dev/null)" = "$target" ]; then
      echo "${dir#/proc/}"
    fi
  done
}

stat_field() {
  sed 's/^.*) //' "/proc/$1/stat" 2>/dev/null | awk -v n="$(($2 - 2))" '{print $n}'
}

alive() {
  [ -n "$1" ] && [ -r "/proc/$1/stat" ] && [ "$(stat_field "$1" 22)" = "$2" ] && [ "$(stat_field "$1" 3)" != "Z" ]
}

wait_for() {
  local deadline=$(($(date +%s) + $1))
  shift
  until "$@"; do
    if [ "$(date +%s)" -ge "$deadline" ]; then
      return 1
    fi
    sleep 0.2
  done
}

host_serving() {
  "$host_bin" session list >/dev/null 2>&1
}

host_gone() {
  [ -z "$(pids_of "$host_real")" ]
}

field() {
  grep -o "\"$2\": \"[^\"]*\"" "$1" | head -n1 | cut -d'"' -f4
}

number() {
  grep -o "\"$2\": -\{0,1\}[0-9]*" "$1" | head -n1 | awk '{print $2}'
}

create() {
  "$host_bin" session create --cwd "$work" --shell "$fixture_bin" -- "$@" >"$work/created.json" 2>"$work/created.err"
  local rc=$?
  sid=$(field "$work/created.json" session)
  spid=$(number "$work/created.json" pid)
  sstart=$(number "$work/created.json" started_at)
  return $rc
}

state_of() {
  "$host_bin" session inspect "$1" >"$work/inspect.json" 2>/dev/null
  field "$work/inspect.json" state
}

exited_with() {
  [ "$(state_of "$1")" = "exited" ] && [ "$(number "$work/inspect.json" code)" = "$2" ]
}

start_host() {
  setsid "$host_bin" serve >>"$work/host.log" 2>&1 </dev/null &
  disown
  wait_for 10 host_serving
}

cleanup() {
  local pid
  for pid in $(pids_of "$host_real") $(pids_of "$fixture_real"); do
    kill -KILL "$pid" 2>/dev/null
  done
  cp "$work"/*.log "$evidence"/ 2>/dev/null
  rm -rf "$work"
}
trap cleanup EXIT

os=$(. /etc/os-release 2>/dev/null && echo "${PRETTY_NAME:-unknown}")
libc=$(ldd --version 2>&1 | head -n1)
container="none"
if [ -e /run/.containerenv ] || [ -e /.dockerenv ]; then
  container="yes"
fi
record ENV info "os=$os; kernel=$(uname -r); arch=$(uname -m); libc=$libc; uid=$(id -u); container=$container; route=$route"
record L01 info "$("$host_bin" --version 2>&1); $("$host_bin" identity 2>&1 | tr -d '\n' | tr -s ' ')"

if ! start_host; then
  record L00 fail "the host did not serve within 10 s: $(tail -n3 "$work/host.log" | tr '\n' ' ')"
  exit 1
fi
endpoint=$(ls "$XDG_RUNTIME_DIR"/paneflow-host-*.sock 2>/dev/null | head -n1)
host_pid=$(pids_of "$host_real" | head -n1)

mode=$(stat -c %a "$endpoint" 2>/dev/null)
if [ "$mode" = "600" ]; then
  record L02 pass "socket $endpoint mode $mode, directory mode $(stat -c %a "$XDG_RUNTIME_DIR")"
else
  record L02 fail "socket $endpoint mode ${mode:-missing}"
fi

create idle
idle_sid=$sid
idle_pid=$spid
idle_start=$sstart
child_sid=$(stat_field "$idle_pid" 6)
child_pgid=$(stat_field "$idle_pid" 5)
host_sid=$(stat_field "$host_pid" 6)
if [ "$child_sid" = "$idle_pid" ] && [ "$child_pgid" = "$idle_pid" ] && [ "$child_sid" != "$host_sid" ]; then
  record L03 pass "child pid $idle_pid leads its own session and group; host $host_pid is in session $host_sid"
else
  record L03 fail "child pid=$idle_pid sid=$child_sid pgid=$child_pgid host sid=$host_sid"
fi

timeout 15 "$host_bin" serve >"$work/second-host.log" 2>&1
rc=$?
if [ "$rc" = "3" ] && host_serving && alive "$idle_pid" "$idle_start"; then
  record L04 pass "a second host for the same home exits $rc: $(tr '\n' ' ' <"$work/second-host.log" | cut -c1-160)"
else
  record L04 fail "second host exit $rc; serving=$(host_serving && echo yes || echo no)"
fi

inode=$(stat -c %i "$endpoint")
timeout 15 "$host_bin" --home "$work/home-other" serve --endpoint "$endpoint" >"$work/occupied.log" 2>&1
rc=$?
if [ "$rc" != "0" ] && [ "$rc" != "124" ] && [ "$(stat -c %i "$endpoint" 2>/dev/null)" = "$inode" ] && host_serving && alive "$idle_pid" "$idle_start"; then
  record L05 pass "a host of another home on the occupied endpoint exits $rc and leaves it: $(tr '\n' ' ' <"$work/occupied.log" | cut -c1-200)"
else
  record L05 fail "occupied endpoint: exit $rc, inode $inode -> $(stat -c %i "$endpoint" 2>/dev/null), serving=$(host_serving && echo yes || echo no)"
fi

create descendants-orphan 1 500
held_sid=$sid
held_root=$spid
sleep 3
held_desc=""
for pid in $(pids_of "$fixture_real"); do
  if tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null | grep -q "descendant-sleep"; then
    held_desc="$pid"
  fi
done
held_desc_start=$(stat_field "$held_desc" 22)
held_state=$(state_of "$held_sid")
if [ -n "$held_desc" ] && [ ! -e "/proc/$held_root/stat" -o "$(stat_field "$held_root" 3)" = "Z" ] && [ "$held_state" != "exited" ]; then
  "$host_bin" session stop "$held_sid" >"$work/held-stop.json" 2>&1
  if wait_for 10 exited_with "$held_sid" 0 || [ "$(state_of "$held_sid")" = "exited" ]; then
    if alive "$held_desc" "$held_desc_start"; then
      record L06 fail "the stop reported an exit while descendant $held_desc still holds the PTY"
    else
      record L06 pass "root $held_root exited while descendant $held_desc held the PTY; the record stayed $held_state until the stop resolved the descendant"
    fi
  else
    record L06 fail "the stop did not resolve the held PTY: $(tr '\n' ' ' <"$work/held-stop.json" | cut -c1-200)"
  fi
else
  record L06 fail "root=$held_root descendant=${held_desc:-none} state=$held_state"
fi

create stream 262144 6
stream_sid=$sid
stream_pid=$spid
for _ in $(seq 1 30); do
  kill -STOP "$host_pid" 2>/dev/null
  sleep 0.05
  kill -CONT "$host_pid" 2>/dev/null
  kill -STOP "$stream_pid" 2>/dev/null
  sleep 0.05
  kill -CONT "$stream_pid" 2>/dev/null
  sleep 0.1
done
if wait_for 30 exited_with "$stream_sid" 0 && host_serving; then
  record L07 pass "30 SIGSTOP/SIGCONT cycles on the host and the streaming child; the stream completed with code 0 and the host kept serving"
else
  record L07 fail "stream state $(state_of "$stream_sid") code $(number "$work/inspect.json" code); serving=$(host_serving && echo yes || echo no)"
fi

desktop_loads=no
if [ -x "$desktop_bin" ] && ldd "$desktop_bin" 2>/dev/null | grep -q "not found"; then
  missing=$(ldd "$desktop_bin" 2>/dev/null | awk '/not found/ {print $1}' | tr '\n' ' ')
  "$desktop_bin" host status >"$work/desktop-missing-libs.log" 2>&1
  rc=$?
  if [ "$rc" != "0" ] && grep -q "error while loading shared libraries" "$work/desktop-missing-libs.log" && host_serving && alive "$idle_pid" "$idle_start"; then
    record L08 pass "desktop exits $rc on missing libraries ($missing); the host and its live session are untouched"
  else
    record L08 fail "desktop exit $rc with missing libraries ($missing)"
  fi
elif [ -x "$desktop_bin" ] && "$desktop_bin" --version >/dev/null 2>&1; then
  desktop_loads=yes
  record L08 info "runtime libraries present: $("$desktop_bin" --version)"
else
  record L08 fail "the desktop binary neither loads nor reports missing libraries"
fi

if [ "$(id -u)" = "0" ]; then
  record L09 unavailable "running as root, which bypasses the permission bits the denied-directory case relies on"
else
  before=$(pids_of "$fixture_real" | wc -l)
  chmod 500 "$sessions_dir"
  create idle
  rc=$?
  denied_err=$(tr '\n' ' ' <"$work/created.err" | cut -c1-200)
  chmod 700 "$sessions_dir"
  sleep 1
  after=$(pids_of "$fixture_real" | wc -l)
  if [ "$rc" != "0" ] && [ "$after" -le "$before" ] && host_serving && alive "$idle_pid" "$idle_start"; then
    if create idle && "$host_bin" session stop "$sid" >/dev/null 2>&1; then
      record L09 pass "create exits $rc on a read-only sessions directory without leaving a child ($denied_err); it succeeds once the directory is writable"
    else
      record L09 fail "create still fails after the directory is writable again"
    fi
  else
    record L09 fail "denied directory: exit $rc, fixtures $before -> $after, serving=$(host_serving && echo yes || echo no)"
  fi
fi

kill -KILL "$host_pid"
wait_for 10 host_gone
sleep 1
survivor=no
if alive "$idle_pid" "$idle_start"; then
  survivor=yes
fi
printf 'not a manifest' >"$sessions_dir/$held_sid.json"
before=$(pids_of "$fixture_real" | wc -l)
if start_host; then
  host_pid=$(pids_of "$host_real" | head -n1)
  "$host_bin" session list >"$work/after-restart.json" 2>&1
  sleep 1
  after=$(pids_of "$fixture_real" | wc -l)
  idle_state=$(state_of "$idle_sid")
  if [ "$after" -le "$before" ] && [ "$idle_state" = "lost" ] && { [ "$survivor" = "no" ] || alive "$idle_pid" "$idle_start"; }; then
    record L10 pass "after SIGKILL of the host (child survived: $survivor) a new host reports the session lost, launches nothing (fixtures $before -> $after), and signals no survivor"
  else
    record L10 fail "after host SIGKILL: state $idle_state, fixtures $before -> $after, survivor $survivor"
  fi
  if grep -q "$stream_sid" "$work/after-restart.json" && ! grep -q "\"session\": \"$held_sid\"" "$work/after-restart.json"; then
    record L11 pass "a corrupt manifest is skipped at startup and the other records load: $(grep -i "$held_sid" "$work/host.log" | tail -n1 | cut -c1-200)"
  else
    record L11 fail "corrupt manifest handling: $(tr '\n' ' ' <"$work/after-restart.json" | cut -c1-200)"
  fi
  "$host_bin" session restart "$idle_sid" >"$work/restart.json" 2>&1
  new_pid=$(number "$work/restart.json" pid)
  new_generation=$(number "$work/restart.json" generation)
  "$host_bin" session stop "$idle_sid" >"$work/restart-stop.json" 2>&1
  stopped=$?
  if [ "$new_generation" = "2" ] && [ -n "$new_pid" ] && [ "$new_pid" != "$idle_pid" ] && [ "$stopped" = "0" ]; then
    record L12 pass "an explicit restart of the lost session starts generation 2 as pid $new_pid without replaying its arguments; the stop settles it"
  else
    record L12 fail "restart: generation ${new_generation:-none}, pid ${new_pid:-none}, stop exit $stopped: $(tr '\n' ' ' <"$work/restart.json" | cut -c1-160)"
  fi
else
  record L10 fail "the host did not restart after SIGKILL"
fi

if [ "$desktop_loads" = "yes" ]; then
  "$desktop_bin" host stop >"$work/desktop-stop-idle.log" 2>&1
  rc=$?
  if [ "$rc" = "0" ] && wait_for 10 host_gone && [ ! -e "$endpoint" ]; then
    setsid bash -c "\"$desktop_bin\" host start >\"$work/desktop-start.json\" 2>&1" </dev/null
    sleep 2
    host_pid=$(pids_of "$host_real" | head -n1)
    host_sid=$(stat_field "$host_pid" 6)
    if [ -n "$host_pid" ] && [ "$host_sid" != "$(stat_field $$ 6)" ] && host_serving; then
      create idle
      "$desktop_bin" host stop >"$work/desktop-stop-live.log" 2>&1
      refused=$?
      if [ "$refused" != "0" ] && host_serving && alive "$spid" "$sstart"; then
        "$host_bin" session stop "$sid" >/dev/null 2>&1
        "$desktop_bin" host stop >>"$work/desktop-stop-idle.log" 2>&1
        if wait_for 10 host_gone && [ ! -e "$endpoint" ]; then
          record L13 pass "paneflow host start detaches the host (session $host_sid) from its launcher; host stop is refused (exit $refused) while a session lives and then stops the host and removes the socket"
        else
          record L13 fail "the host did not exit after an idle host stop"
        fi
      else
        record L13 fail "host stop with a live session exited $refused"
      fi
    else
      record L13 fail "the detached host is missing or shares the launcher session: $(tr '\n' ' ' <"$work/desktop-start.json" | cut -c1-200)"
    fi
  else
    record L13 fail "idle host stop exited $rc"
  fi
else
  for pid in $(pids_of "$host_real"); do
    kill -TERM "$pid" 2>/dev/null
  done
  record L13 unavailable "the desktop binary cannot load here, so paneflow host start and stop are not exercised"
fi

sleep 1
leftover=$(pids_of "$fixture_real" | tr '\n' ' ')
if [ -z "$leftover" ]; then
  record L14 pass "no fixture process of this run survives"
else
  record L14 fail "fixture processes still alive: $leftover"
fi

if [ "$failures" -gt 0 ]; then
  exit 1
fi
