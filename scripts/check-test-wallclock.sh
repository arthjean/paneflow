#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
ALLOWLIST="$ROOT/scripts/wallclock-allowlist.txt"

mode=check
case "${1:-}" in
  "") ;;
  --inventory) mode=inventory ;;
  *)
    echo "usage: scripts/check-test-wallclock.sh [--inventory]" >&2
    exit 2
    ;;
esac

python=""
for candidate in python3 python; do
  if command -v "$candidate" >/dev/null 2>&1 && "$candidate" -c 'import sys; sys.exit(sys.version_info < (3, 8))' 2>/dev/null; then
    python="$candidate"
    break
  fi
done
if [ -z "$python" ]; then
  echo "::error::scripts/check-test-wallclock.sh needs Python 3.8 or later on PATH" >&2
  exit 1
fi

exec "$python" - "$ROOT" "$ALLOWLIST" "$mode" <<'PY'
import os
import re
import sys

root, allowlist_path, mode = sys.argv[1], sys.argv[2], sys.argv[3]
scan_roots = ["crates", "src-app"]
string_literal = re.compile(r'r#*"(?:.|\n)*?"#*|"(?:[^"\\]|\\.)*"')
function = re.compile(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)")
elapsed_binding = re.compile(r"\b(?:let\s+(?:mut\s+)?)?([a-z_][a-z0-9_]*)\s*(?::[^=]+)?=[^=].*\.elapsed\(\)")
percentile_binding = re.compile(r"\blet\s+(?:mut\s+)?\(?\s*([a-z_][a-z0-9_]*(?:\s*,\s*[a-z_][a-z0-9_]*)*)\s*\)?\s*(?::[^=]+)?=[^=].*\b(?:p[0-9]{2}|percentile)\s*\(")
assertion_start = re.compile(r"\b(?:debug_)?assert!\s*\(")
comparison = re.compile(r"(?<![-=<>])(?:<=|>=|<|>)(?![=<>])")
duration_side = re.compile(r"\bDuration\b|\b[A-Z][A-Z0-9_]{2,}\b")
test_path = re.compile(r"(^|/)tests?(/|\.rs$)|_tests?(/|\.rs$)|(^|/)tests_[^/]*\.rs$")
test_section = re.compile(r"^\s*#\[cfg\((?:all\()?test\b")


def test_lines(relative, lines):
    if test_path.search(relative):
        return 0
    for index, line in enumerate(lines):
        if test_section.match(line):
            return index
    return None


def strip_strings(text):
    return string_literal.sub('""', text)


def assertion_text(lines, start):
    depth = 0
    collected = []
    opened = False
    for index in range(start, min(start + 40, len(lines))):
        line = strip_strings(lines[index])
        if index == start:
            line = line[assertion_start.search(line).start():]
        for char in line:
            collected.append(char)
            if char == "(":
                depth += 1
                opened = True
            elif char == ")":
                depth -= 1
                if opened and depth == 0:
                    return "".join(collected)
        collected.append(" ")
    return "".join(collected)


def compares_elapsed(text, names):
    operands = [r"\.elapsed\(\)", r"\b[a-z_0-9]*elapsed[a-z_0-9]*\b"]
    operands += [r"\b" + re.escape(name) + r"\b" for name in sorted(names)]
    operand = re.compile("|".join(operands))
    for part in re.split(r",(?![^(]*\))", text):
        if operand.search(part) and comparison.search(part) and duration_side.search(part):
            return True
    return False


found = {}
for scan_root in scan_roots:
    for directory, subdirectories, files in os.walk(os.path.join(root, scan_root)):
        subdirectories[:] = sorted(d for d in subdirectories if d not in ("target", "node_modules"))
        for name in sorted(files):
            if not name.endswith(".rs"):
                continue
            path = os.path.join(directory, name)
            relative = os.path.relpath(path, root).replace(os.sep, "/")
            with open(path, encoding="utf-8", errors="replace") as handle:
                lines = handle.read().splitlines()
            first = test_lines(relative, lines)
            if first is None:
                continue
            current = "<module>"
            names = set()
            for index in range(first, len(lines)):
                code = strip_strings(lines[index])
                if code.lstrip().startswith("//"):
                    continue
                declared = function.search(code)
                if declared:
                    current = declared.group(1)
                    names = set()
                for bound in (elapsed_binding.search(code), percentile_binding.search(code)):
                    if bound:
                        names.update(name.strip() for name in bound.group(1).split(","))
                if assertion_start.search(code) and compares_elapsed(assertion_text(lines, index), names):
                    found.setdefault((relative, current), []).append(index + 1)

if mode == "inventory":
    for (relative, current), occurrences in sorted(found.items()):
        print(f"{relative} {current} {len(occurrences)}")
    sys.exit(0)

allowed = {}
with open(allowlist_path, encoding="utf-8") as handle:
    for number, raw in enumerate(handle, 1):
        entry = raw.strip()
        if not entry:
            continue
        fields = entry.split()
        if len(fields) != 3 or not fields[2].isdigit():
            print(f"::error file=scripts/wallclock-allowlist.txt,line={number}::scripts/wallclock-allowlist.txt:{number}: expected '<path> <test function> <count>', got '{entry}'")
            sys.exit(1)
        allowed[(fields[0], fields[1])] = int(fields[2])

status = 0
for key, occurrences in sorted(found.items()):
    relative, current = key
    limit = allowed.get(key, 0)
    if len(occurrences) <= limit:
        continue
    status = 1
    for line in occurrences[limit:] if limit else occurrences:
        print(
            f"::error file={relative},line={line}::{relative}:{line}: a unit test compares elapsed() or a percentile to a Duration in {current}. "
            "Assert the outcome under a generous timeout and move the latency budget to the perf gates "
            "(scripts/perf-gates.sh, crates/paneflow-host/tests/persistent_baseline/gates.rs)"
        )
for key, limit in sorted(allowed.items()):
    count = len(found.get(key, []))
    if count < limit:
        status = 1
        relative, current = key
        print(
            f"::error file=scripts/wallclock-allowlist.txt::scripts/wallclock-allowlist.txt: {relative} {current} allows {limit} "
            f"wall-clock assertion(s) but {count} remain; lower the entry to {count}" + (" or remove it" if count == 0 else "")
        )

if status == 0:
    total = sum(len(occurrences) for occurrences in found.values())
    print(f"no new wall-clock assertion in test code ({total} allowlisted in {len(found)} test function(s))")
sys.exit(status)
PY
