#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: scripts/ci-test-report.sh <nextest junit.xml>" >&2
  exit 2
fi

python=""
for candidate in python3 python; do
  if command -v "$candidate" >/dev/null 2>&1 && "$candidate" -c 'import sys; sys.exit(sys.version_info < (3, 8))' 2>/dev/null; then
    python="$candidate"
    break
  fi
done
if [ -z "$python" ]; then
  echo "::error::scripts/ci-test-report.sh needs Python 3.8 or later on PATH" >&2
  exit 1
fi

exec "$python" - "$1" <<'PY'
import os
import sys
import xml.etree.ElementTree as ET

path = sys.argv[1]
if not os.path.isfile(path):
    print(f"::notice::no nextest JUnit report at {path}: the test step did not reach the test run")
    sys.exit(0)

root = ET.parse(path).getroot()


def annotation_text(text):
    return text.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")


def annotation_property(text):
    return annotation_text(text).replace(":", "%3A").replace(",", "%2C")


def rerun(binary, name):
    return f"cargo nextest run --locked --profile ci -E 'binary_id({binary}) & test(={name})'"


rows = []
for case in root.iter("testcase"):
    binary = case.get("classname", "")
    name = case.get("name", "")
    seconds = float(case.get("time") or 0)
    failure = case.find("failure")
    reruns = case.findall("rerunFailure")
    flaky = case.findall("flakyFailure")
    if failure is None and not flaky:
        continue
    kind = failure.get("type", "") if failure is not None else ""
    if failure is None:
        level, outcome = "warning", f"is FLAKY and quarantined: failed {len(flaky)} time(s), then passed"
    elif kind == "flaky failure":
        level, outcome = "error", "is FLAKY: failed then passed. Fix it or add it to the quarantine with an issue"
    elif kind == "test timeout":
        level, outcome = "error", f"TIMEOUT after {seconds:.0f} s"
    elif reruns:
        level, outcome = "error", f"failed all {len(reruns) + 1} attempts"
    else:
        level, outcome = "error", "failed"
    command = rerun(binary, name)
    title = annotation_property(f"{binary} {name}")
    print(f"::{level} title={title}::{annotation_text(f'{name} {outcome}. Rerun: {command}')}")
    rows.append((level, binary, name, outcome, command))

tests = root.get("tests", "?")
failures = root.get("failures", "?")
summary = os.environ.get("GITHUB_STEP_SUMMARY")
if summary:
    with open(summary, "a", encoding="utf-8") as handle:
        handle.write(f"### nextest: {tests} tests, {failures} failed\n\n")
        if rows:
            handle.write("| Result | Binary | Test | Outcome | Rerun |\n|---|---|---|---|---|\n")
            for level, binary, name, outcome, command in rows:
                result = "FAIL" if level == "error" else "QUARANTINED FLAKY"
                handle.write(f"| {result} | `{binary}` | `{name}` | {outcome} | `{command}` |\n")
            handle.write("\n")
print(f"nextest report: {tests} tests, {failures} failed, {len(rows)} annotated")
PY
