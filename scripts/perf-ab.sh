#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
root=$(pwd -P)

if [ $# -ne 2 ]; then
  echo "usage: scripts/perf-ab.sh <base-commit> <head-commit>" >&2
  exit 2
fi
base_ref=$1
head_ref=$2
rounds=10
cargo=${CARGO:-cargo}
timed_editor=${PERF_AB_TIMED_EDITOR:-1}
ab_profile=${PERF_AB_PROFILE:-gates}
case "$ab_profile" in
  gates) strip_variable=CARGO_PROFILE_GATES_STRIP ;;
  release) strip_variable=CARGO_PROFILE_RELEASE_STRIP ;;
  *)
    echo "PERF_AB_PROFILE is $ab_profile: set it to gates, the profile of the CI job, or release, the profile Paneflow ships" >&2
    exit 2
    ;;
esac
case "$timed_editor" in
  0 | 1) ;;
  *)
    echo "PERF_AB_TIMED_EDITOR is $timed_editor: set it to 1 to time the editor suite in every slot, or 0 to leave it to the instruction counts" >&2
    exit 2
    ;;
esac

for variable in $(compgen -e | grep '^PANEFLOW_' || true); do
  case "$variable" in
    PANEFLOW_PERF_AB_DIR) ;;
    *) unset "$variable" ;;
  esac
done

base_sha=$(git rev-parse --verify --quiet "$base_ref^{commit}") || {
  echo "base $base_ref is not a commit" >&2
  exit 2
}
head_sha=$(git rev-parse --verify --quiet "$head_ref^{commit}") || {
  echo "head $head_ref is not a commit" >&2
  exit 2
}
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  checkout_sha=$(git rev-parse HEAD)
  for side in base head; do
    sha_variable="${side}_sha"
    if [ "${!sha_variable}" = "$checkout_sha" ]; then
      echo "the working tree has uncommitted changes and $side resolves to its HEAD commit: the A/B measures commits, never a dirty tree; commit the changes first (git status --short)" >&2
      exit 2
    fi
  done
fi

out="${PANEFLOW_PERF_AB_DIR:-$root/target/perf-ab}"
mkdir -p "$out"
rm -rf "$out"/attempt-* "$out"/build-*.log "$out"/result.json "$out"/summary.md "$out"/verdict "$out"/attempt-verdict "$out"/instructions*
scratch=$(mktemp -d "${TMPDIR:-/tmp}/paneflow-perf-ab.XXXXXX")
if [ -z "${CARGO_TARGET_DIR:-}" ]; then
  export CARGO_TARGET_DIR="$scratch/target"
fi

finish() {
  deliberate=true
  exit "$1"
}

cleanup() {
  local status=$?
  if [ "${deliberate:-false}" != true ] && [ "$status" -ne 0 ]; then
    echo "the A/B stopped on an unexpected error (exit $status)" >&2
    status=2
  fi
  for side in base head; do
    if [ -d "$scratch/$side" ]; then
      git worktree remove --force "$scratch/$side" >/dev/null 2>&1 || true
    fi
  done
  git worktree prune
  rm -rf "$scratch"
  exit "$status"
}
trap cleanup EXIT
trap 'finish 130' INT TERM

host_triple=$(rustc -vV | sed -n 's/^host: //p')
started=$(date +%s)

unavailable() {
  local side=$1 reason=$2 log=$3 code=3
  local sha_variable="${side}_sha"
  if [ "$side" = head ]; then
    code=4
  fi
  echo "$side ${!sha_variable:0:12} unavailable: $reason" >&2
  grep -E '^error' -A 6 "$log" | head -n 60 >&2 || true
  echo "log: $log" >&2
  finish "$code"
}

executable() {
  grep -o '"executable":"[^"]*/'"$2"'-[^"/]*"' "$1" | tail -n 1 | sed 's/^"executable":"//; s/"$//'
}

prepare() {
  local side=$1 sha=$2
  local worktree="$scratch/$side"
  git worktree add --detach --quiet "$worktree" "$sha"
  git ls-files --others --ignored --exclude-standard --directory -- native | while IFS= read -r ignored; do
    if [ -d "$root/$ignored" ]; then
      mkdir -p "$worktree/$ignored"
      cp -R "$root/$ignored." "$worktree/$ignored"
    else
      mkdir -p "$(dirname "$worktree/$ignored")"
      cp "$root/$ignored" "$worktree/$ignored"
    fi
  done
}

build() {
  local side=$1
  local worktree="$scratch/$side" bin="$scratch/bin-$side" log="$out/build-$side.log"
  mkdir -p "$bin"
  echo "== building $side $(git rev-parse --short=12 "$2") in the $ab_profile profile"
  set +e
  (
    set -e
    cd "$worktree"
    export "$strip_variable=false"
    bash scripts/fetch-libghostty.sh --target "$host_triple"
    "$cargo" build --profile "$ab_profile" --locked -p paneflow-host
    "$cargo" test --profile "$ab_profile" --locked -p paneflow-app --bin paneflow --no-run \
      --message-format=json-render-diagnostics >"$bin/terminal.jsonl"
    if [ "$side" = head ]; then
      "$cargo" test --profile "$ab_profile" --locked -p paneflow-host --test persistent_baseline --no-run \
        --message-format=json-render-diagnostics >"$bin/harness.jsonl"
    fi
  ) >"$log" 2>&1
  local status=$?
  set -e
  if [ "$status" -ne 0 ]; then
    unavailable "$side" "the build failed" "$log"
  fi
  local terminal
  terminal=$(executable "$bin/terminal.jsonl" paneflow)
  if [ ! -x "$terminal" ]; then
    unavailable "$side" "the terminal suite executable was not built" "$log"
  fi
  cp "$terminal" "$bin/terminal-suite"
  cp "$CARGO_TARGET_DIR/$ab_profile/paneflow-host" "$bin/paneflow-host"
  if [ "$side" = head ]; then
    cp "$CARGO_TARGET_DIR/$ab_profile/paneflow-session-fixture" "$bin/paneflow-session-fixture"
    local harness
    harness=$(executable "$bin/harness.jsonl" persistent_baseline)
    if [ ! -x "$harness" ]; then
      unavailable head "the persistent_baseline harness was not built" "$log"
    fi
    cp "$harness" "$bin/harness"
  fi
}

run_slot() {
  local dir=$1 round=$2 slot=$3 side=$4
  local bin="$scratch/bin-$side" tag status
  tag=$(printf 'r%02d-%d-%s' "$round" "$slot" "$side")
  set +e
  (
    cd "$scratch/$side/src-app"
    PANEFLOW_BENCH_SKIP_IDLE=1 PANEFLOW_BENCH_SAMPLES_OUT="$dir/$tag-terminal.json" \
      "$bin/terminal-suite" terminal::perf_bench::terminal_pipeline_benchmark \
      --ignored --exact --nocapture --test-threads=1
  ) >"$dir/$tag-terminal.log" 2>&1
  status=$?
  set -e
  if [ "$status" -ne 0 ] || [ ! -s "$dir/$tag-terminal.json" ]; then
    unavailable "$side" "the terminal suite exited $status without samples in round $round; a commit older than the A/B sample export writes none" "$dir/$tag-terminal.log"
  fi
  if [ "$timed_editor" = 1 ]; then
    set +e
    (
      cd "$scratch/$side/src-app"
      PANEFLOW_BENCH_SAMPLES_OUT="$dir/$tag-editor.json" \
        "$bin/terminal-suite" app::diff_dock::code::perf_bench::editor_pipeline_benchmark \
        --ignored --exact --nocapture --test-threads=1
    ) >"$dir/$tag-editor.log" 2>&1
    status=$?
    set -e
    if [ "$status" -ne 0 ] || [ ! -s "$dir/$tag-editor.json" ]; then
      unavailable "$side" "the editor suite exited $status without samples in round $round" "$dir/$tag-editor.log"
    fi
  fi
  set +e
  (
    cd "$scratch/head/crates/paneflow-host"
    PANEFLOW_BENCH_HOST="$bin/paneflow-host" \
      PANEFLOW_BENCH_FIXTURE="$scratch/bin-head/paneflow-session-fixture" \
      PANEFLOW_AB_SAMPLES_OUT="$dir/$tag-active.json" \
      "$scratch/bin-head/harness" ab::perf_ab_active_samples \
      --ignored --exact --nocapture --test-threads=1
  ) >"$dir/$tag-active.log" 2>&1
  status=$?
  set -e
  if [ "$status" -ne 0 ] || [ ! -s "$dir/$tag-active.json" ]; then
    unavailable "$side" "the active scenario exited $status without samples in round $round" "$dir/$tag-active.log"
  fi
}

measure() {
  local attempt=$1
  local dir="$out/attempt-$attempt"
  mkdir -p "$dir"
  for round in $(seq 1 "$rounds"); do
    echo "== attempt $attempt, round $round of $rounds"
    run_slot "$dir" "$round" 1 base
    run_slot "$dir" "$round" 2 head
    run_slot "$dir" "$round" 3 head
    run_slot "$dir" "$round" 4 base
  done
}

compare() {
  (
    cd "$scratch/head/crates/paneflow-host"
    PANEFLOW_AB_DIR="$out" \
      PANEFLOW_AB_BASE_REF="$base_ref" PANEFLOW_AB_BASE_SHA="$base_sha" \
      PANEFLOW_AB_HEAD_REF="$head_ref" PANEFLOW_AB_HEAD_SHA="$head_sha" \
      "$scratch/bin-head/harness" ab::perf_ab_compare --ignored --exact --nocapture --test-threads=1
  )
}

instruction_benches=(
  "paneflow-terminal-ghostty|native"
)
instruction_limit_percent=2
editor_instruction_function=paneflow_bench_counted_instructions

count_editor_instructions_side() {
  local side=$1 status=0
  local dumps="$scratch/callgrind-$side" log="$out/instructions-$side-editor.log"
  rm -rf "$dumps"
  mkdir -p "$dumps"
  set +e
  (
    cd "$scratch/$side/src-app"
    valgrind --tool=callgrind --collect-atstart=no \
      "--toggle-collect=$editor_instruction_function" \
      "--dump-after=$editor_instruction_function" \
      "--callgrind-out-file=$dumps/callgrind.out" \
      "$scratch/bin-$side/terminal-suite" app::diff_dock::code::perf_bench::editor_instruction_benchmark \
      --ignored --exact --nocapture --test-threads=1
  ) >"$log" 2>&1
  status=$?
  set -e
  python3 - "$dumps" "$log" "$out/instructions-$side-editor.json" "$status" <<'PY'
import json
import pathlib
import re
import sys

dumps, log, target, status = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), pathlib.Path(sys.argv[3]), int(sys.argv[4])
names = re.findall(r"PANEFLOW_BENCH_INSTRUCTION_CASE (\S+)", log.read_text(errors="replace"))
files = sorted((path for path in dumps.glob("callgrind.out.*") if re.fullmatch(r"callgrind\.out\.\d+", path.name)), key=lambda path: int(path.suffix[1:]))
counts = []
for path in files:
    totals = [line for line in path.read_text(errors="replace").splitlines() if line.startswith(("summary:", "totals:"))]
    counts.append(int(totals[-1].split()[1]) if totals else 0)
report = {"status": status, "cases": {}, "reason": None}
if status != 0:
    report["reason"] = f"the editor instruction suite exited {status}"
elif len(names) != len(counts):
    report["reason"] = f"{len(names)} editor cases ran but Callgrind wrote {len(counts)} dumps"
else:
    report["cases"] = dict(zip(names, counts))
target.write_text(json.dumps(report, indent=2) + "\n")
PY
}

count_instructions_side() {
  local side=$1 status=0
  local log="$out/instructions-$side.log" json="$out/instructions-$side.jsonl"
  local mode=(--save-baseline=base)
  if [ "$side" = head ]; then
    mode=(--baseline=base "--callgrind-limits=ir=${instruction_limit_percent}%")
  fi
  : >"$json"
  for entry in "${instruction_benches[@]}"; do
    local package=${entry%%|*} features=${entry#*|}
    local args=(bench --locked -q -p "$package" --bench instructions)
    if [ -n "$features" ]; then
      args+=(--features "$features")
    fi
    set +e
    (
      cd "$scratch/$side"
      "$cargo" "${args[@]}" -- --home="$scratch/gungraun" --output-format=json "${mode[@]}"
    ) >>"$json" 2>>"$log"
    status=$?
    set -e
    if [ "$status" -ne 0 ] && { [ "$side" = base ] || [ "$status" -ne 3 ]; }; then
      return 1
    fi
  done
}

count_instructions() {
  local reason=""
  if ! command -v valgrind >/dev/null 2>&1; then
    reason="valgrind is not installed"
  elif ! command -v gungraun-runner >/dev/null 2>&1; then
    reason="gungraun-runner is not installed (cargo install gungraun-runner --version <the gungraun version in Cargo.lock>)"
  else
    echo "== counting instructions"
    if ! count_instructions_side base; then
      reason="the base has no instruction benchmarks or they failed; see instructions-base.log"
    elif ! count_instructions_side head; then
      reason="the head instruction benchmarks failed; see instructions-head.log"
    fi
    echo "== counting editor instructions"
    count_editor_instructions_side base
    count_editor_instructions_side head
  fi
  python3 - "$out" "$instruction_limit_percent" "$reason" <<'PY'
import json
import pathlib
import sys

out = pathlib.Path(sys.argv[1])
limit = float(sys.argv[2])
reason = sys.argv[3]
benchmarks = []
if not reason:
    for line in (out / "instructions-head.jsonl").read_text().splitlines():
        if not line.startswith("{"):
            continue
        record = json.loads(line)
        parts = record["profiles"][0]["data"]["parts"]
        values = [part["metrics"]["Ir"]["values"] for part in parts]
        base = sum(value.get("old") or 0 for value in values)
        head = sum(value.get("new") or 0 for value in values)
        name = f"{record['group']}::{record['function_name']}.{record['id']}"
        delta = (head - base) / base * 100 if base else None
        benchmarks.append({
            "name": name,
            "base_ir": base or None,
            "head_ir": head,
            "delta_percent": delta,
            "verdict": "not_compared" if delta is None else ("regression" if delta > limit else "pass"),
        })
    if not benchmarks:
        reason = "the instruction benchmarks reported nothing"
editor = {}
for side in ("base", "head"):
    path = out / f"instructions-{side}-editor.json"
    editor[side] = json.loads(path.read_text()) if path.exists() else {"cases": {}, "reason": "the editor instruction suite did not run"}
editor_reason = editor["head"].get("reason")
if editor_reason:
    reason = "; ".join(part for part in (reason, f"head: {editor_reason}, see instructions-head-editor.log") if part)
for case, head in editor["head"].get("cases", {}).items():
    base = editor["base"].get("cases", {}).get(case)
    delta = (head - base) / base * 100 if base else None
    benchmarks.append({
        "name": f"editor::{case}",
        "base_ir": base or None,
        "head_ir": head,
        "delta_percent": delta,
        "verdict": "not_compared" if delta is None else ("regression" if delta > limit else "pass"),
    })
if any(entry["verdict"] == "regression" for entry in benchmarks):
    verdict = "regression"
elif reason:
    verdict = "not_measured"
else:
    verdict = "pass"
report = {"verdict": verdict, "limit_percent": limit, "reason": reason or None, "benchmarks": benchmarks}
(out / "instructions.json").write_text(json.dumps(report, indent=2) + "\n")
(out / "instructions-verdict").write_text(verdict + "\n")
lines = ["", "## Instruction counts", "", f"Verdict: **{verdict}** (Callgrind `Ir`, soft limit +{limit:g} % against the base built in this run)"]
if reason:
    lines += ["", f"Not measured: {reason}."]
if benchmarks:
    lines += ["", "| Benchmark | Base Ir | Head Ir | Delta | Verdict |", "|---|---|---|---|---|"]
    for entry in benchmarks:
        delta = "n/a" if entry["delta_percent"] is None else f"{entry['delta_percent']:+.3f} %"
        lines.append(f"| `{entry['name']}` | {entry['base_ir'] or 'n/a'} | {entry['head_ir']} | {delta} | {entry['verdict']} |")
(out / "instructions.md").write_text("\n".join(lines) + "\n")
PY
}

prepare base "$base_sha"
build base "$base_sha"
prepare head "$head_sha"
build head "$head_sha"

count_instructions

measure 1
compare
first=$(cat "$out/attempt-verdict")
if [ "$first" = regression ] || [ "$first" = uncalibrated ]; then
  echo "== the first execution was $first; measuring once more before any verdict"
  measure 2
  compare
fi

cat "$out/instructions.md" >>"$out/summary.md"
verdict=$(cat "$out/verdict")
instructions=$(cat "$out/instructions-verdict")
echo "A/B finished in $(( $(date +%s) - started )) s: $verdict, instructions $instructions; report $out/result.json, summary $out/summary.md"
case "$verdict" in
  pass | unconfirmed_regression)
    if [ "$instructions" = regression ]; then
      finish 1
    fi
    finish 0
    ;;
  regression) finish 1 ;;
  uncalibrated) finish 5 ;;
  *) finish 2 ;;
esac
