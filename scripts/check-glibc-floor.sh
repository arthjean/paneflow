#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -lt 2 ]; then
  echo "usage: check-glibc-floor.sh <highest allowed glibc version, e.g. 2.35> <elf binary>..." >&2
  exit 2
fi

FLOOR="$1"
shift
OBJDUMP="${OBJDUMP:-objdump}"

if ! [[ "$FLOOR" =~ ^([0-9]+)\.([0-9]+)$ ]]; then
  echo "::error::the glibc floor must look like 2.35, not '$FLOOR'" >&2
  exit 2
fi
FLOOR_MAJOR="${BASH_REMATCH[1]}"
FLOOR_MINOR="${BASH_REMATCH[2]}"

status=0
for binary in "$@"; do
  if [ ! -f "$binary" ]; then
    echo "::error::$binary does not exist, so its glibc symbol versions cannot be read"
    status=1
    continue
  fi
  if ! symbols="$("$OBJDUMP" -T "$binary" 2>&1)"; then
    echo "::error::$OBJDUMP -T $binary failed: ${symbols//$'\n'/ }"
    status=1
    continue
  fi
  report="$(
    awk -v major="$FLOOR_MAJOR" -v minor="$FLOOR_MINOR" '
      {
        for (i = 1; i <= NF; i++) {
          field = $i
          gsub(/[()]/, "", field)
          if (field !~ /^GLIBC_[0-9]+\.[0-9]+/) continue
          version = substr(field, 7)
          split(version, part, ".")
          found = 1
          if (part[1] + 0 > high_major || (part[1] + 0 == high_major && part[2] + 0 > high_minor)) {
            high_major = part[1] + 0
            high_minor = part[2] + 0
          }
          if (part[1] + 0 > major || (part[1] + 0 == major && part[2] + 0 > minor)) {
            print "over GLIBC_" version " " $NF
          }
        }
      }
      END {
        if (found) print "highest " high_major "." high_minor
      }
    ' <<<"$symbols" | LC_ALL=C sort -u
  )"
  highest="$(sed -n 's/^highest //p' <<<"$report")"
  if [ -z "$highest" ]; then
    echo "::error::$binary references no GLIBC_ symbol version: it is not a dynamically linked glibc binary, and the floor check read nothing"
    status=1
    continue
  fi
  over=0
  while IFS=' ' read -r kind version symbol; do
    [ "$kind" = over ] || continue
    echo "::error::$binary needs $version for $symbol, above the glibc $FLOOR floor: it would not start on a distribution that ships glibc $FLOOR"
    over=1
  done <<<"$report"
  if ((over == 1)); then
    status=1
  else
    echo "$binary: highest glibc symbol version $highest (floor $FLOOR)"
  fi
done

exit "$status"
