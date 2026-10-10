#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

out=target/gungraun-spike
max_drift_percent=0.1
rm -rf "$out"
mkdir -p "$out"

benches=(
  "paneflow-terminal-ghostty|native"
)

failed=""
for run in 1 2; do
  : > "$out/run-$run.jsonl"
  for entry in "${benches[@]}"; do
    package=${entry%%|*}
    features=${entry#*|}
    args=(bench --locked -q -p "$package" --bench instructions)
    if [ -n "$features" ]; then
      args+=(--features "$features")
    fi
    echo "== run $run: $package"
    if ! cargo "${args[@]}" -- --output-format=json >> "$out/run-$run.jsonl" 2>> "$out/run-$run.log"; then
      failed="${failed:+$failed, }run $run $package"
    fi
  done
done

python3 - "$out" "$max_drift_percent" "$failed" <<'PY'
import json
import pathlib
import sys

out = pathlib.Path(sys.argv[1])
max_drift = float(sys.argv[2])
failed = sys.argv[3].strip()
expected = {
    "terminal::parse_and_convert.mebibyte_220x60",
}


def instructions(run):
    counts = {}
    path = out / f"run-{run}.jsonl"
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        record = json.loads(line)
        name = f"{record['group']}::{record['function_name']}.{record['id']}"
        parts = record["profiles"][0]["data"]["parts"]
        counts[name] = sum(part["metrics"]["Ir"]["values"]["new"] for part in parts)
    return counts


first, second = instructions(1), instructions(2)
rows = []
reasons = []
if failed:
    reasons.append(f"a benchmark did not complete: {failed}")
for name in sorted(expected):
    a, b = first.get(name), second.get(name)
    if a is None or b is None:
        reasons.append(f"{name} reported no instruction count")
        rows.append((name, a, b, None))
        continue
    if not a or not b:
        reasons.append(f"{name} counted zero instructions, so Callgrind never saw the benchmark function")
        rows.append((name, a, b, None))
        continue
    drift = abs(b - a) / a * 100
    if drift > max_drift:
        reasons.append(f"{name} drifts by {drift:.4f} % between two runs of the same commit, above {max_drift} %")
    rows.append((name, a, b, drift))

verdict = "validated" if not reasons else "not validated"
lines = [
    f"Verdict: **{verdict}**",
    "",
    "Layout 220x60 is out of scope: it lives in the paneflow-app binary crate, which has no library target.",
    "",
    "| Benchmark | Run 1 (Ir) | Run 2 (Ir) | Drift |",
    "|---|---|---|---|",
]
for name, a, b, drift in rows:
    shown = "n/a" if drift is None else f"{drift:.4f} %"
    lines.append(f"| `{name}` | {a if a is not None else 'missing'} | {b if b is not None else 'missing'} | {shown} |")
if reasons:
    lines += ["", "Reasons:", ""] + [f"- {reason}" for reason in reasons]
    log = out / "run-1.log"
    if failed and log.exists():
        tail = log.read_text(errors="replace").splitlines()[-40:]
        lines += ["", "```", *tail, "```"]
(out / "summary.md").write_text("\n".join(lines) + "\n")
(out / "verdict").write_text(verdict + "\n")
(out / "result.json").write_text(json.dumps({
    "verdict": verdict,
    "max_drift_percent": max_drift,
    "runs": [first, second],
    "reasons": reasons,
}, indent=2) + "\n")
print("\n".join(lines))
sys.exit(0 if verdict == "validated" else 1)
PY
