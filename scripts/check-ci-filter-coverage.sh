#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "${1:-$(dirname "${BASH_SOURCE[0]}")/..}" && pwd -P)"
WORKFLOW="$ROOT/.github/workflows/run_tests.yml"
FILTER_NAME=rust
EXTRACT_RE='(include_str|include_bytes|include)![[:space:]]*\([[:space:]]*"([^"]*)"|#\[[[:space:]]*folder[[:space:]]*=[[:space:]]*"([^"]*)"|cargo::?rerun-if-changed=([^"{}]*)"'

if [ ! -f "$WORKFLOW" ]; then
  echo "::error::$WORKFLOW does not exist"
  exit 1
fi

patterns=()
in_filter=0
while IFS= read -r line || [ -n "$line" ]; do
  line="${line%$'\r'}"
  if [[ "$line" =~ ^[[:space:]]{12}$FILTER_NAME:[[:space:]]*$ ]]; then
    in_filter=1
    continue
  fi
  [ "$in_filter" -eq 1 ] || continue
  if [[ "$line" =~ ^[[:space:]]+-[[:space:]]+[\'\"]?([^\'\"]+)[\'\"]?[[:space:]]*$ ]]; then
    patterns+=("${BASH_REMATCH[1]}")
  elif [[ "$line" =~ ^[[:space:]]*# || -z "${line//[[:space:]]/}" ]]; then
    continue
  else
    break
  fi
done < "$WORKFLOW"

if [ "${#patterns[@]}" -eq 0 ]; then
  echo "::error::no '$FILTER_NAME' path filter found in .github/workflows/run_tests.yml"
  exit 1
fi

glob_regex() {
  local glob="$1" out="" char next
  local i=0
  while [ "$i" -lt "${#glob}" ]; do
    char="${glob:$i:1}"
    next="${glob:$((i + 1)):1}"
    if [ "$char" = "*" ] && [ "$next" = "*" ]; then
      if [ "${glob:$((i + 2)):1}" = "/" ]; then
        out+="(.*/)?"
        i=$((i + 3))
      else
        if [ "${out: -1}" = "/" ]; then
          out="${out%/}(/.*)?"
        else
          out+=".*"
        fi
        i=$((i + 2))
      fi
      continue
    fi
    case "$char" in
      "*") out+="[^/]*" ;;
      "?") out+="[^/]" ;;
      [.+\(\)\|\^\$\{\}\[\]\\]) out+="\\$char" ;;
      *) out+="$char" ;;
    esac
    i=$((i + 1))
  done
  printf '^%s$' "$out"
}

regexes=()
for pattern in "${patterns[@]}"; do
  regexes+=("$(glob_regex "$pattern")")
done

covered() {
  local regex
  for regex in "${regexes[@]}"; do
    if [[ "$1" =~ $regex ]]; then
      return 0
    fi
  done
  return 1
}

normalize() {
  local -a parts=() stack=()
  local part
  IFS=/ read -r -a parts <<< "$1"
  for part in "${parts[@]}"; do
    case "$part" in
      "" | .) ;;
      ..)
        if [ "${#stack[@]}" -eq 0 ]; then
          return 1
        fi
        unset 'stack[${#stack[@]}-1]'
        ;;
      *) stack+=("$part") ;;
    esac
  done
  local IFS=/
  printf '%s' "${stack[*]}"
}

crate_dir() {
  local dir="$1"
  while [ -n "$dir" ] && [ "$dir" != . ]; do
    if [ -f "$ROOT/$dir/Cargo.toml" ]; then
      printf '%s' "$dir"
      return
    fi
    dir="$(dirname "$dir")"
  done
  printf '.'
}

mapfile -t sources < <(git -C "$ROOT" ls-files --cached --others --exclude-standard -- '*.rs' | sort -u)
if [ "${#sources[@]}" -eq 0 ]; then
  echo "::error::no Rust source found under $ROOT"
  exit 1
fi

status=0
checked=0
for source in "${sources[@]}"; do
  [ -f "$ROOT/$source" ] || continue
  text="$(tr '\r\n' '  ' < "$ROOT/$source")"
  while [[ "$text" =~ $EXTRACT_RE ]]; do
    match="${BASH_REMATCH[0]}"
    text="${text#*"$match"}"
    if [ -n "${BASH_REMATCH[2]}" ]; then
      cited="${BASH_REMATCH[2]}"
      base="$(dirname "$source")"
    elif [ -n "${BASH_REMATCH[3]}" ]; then
      cited="${BASH_REMATCH[3]}"
      base="$(crate_dir "$(dirname "$source")")"
    else
      cited="${BASH_REMATCH[4]}"
      base="$(crate_dir "$(dirname "$source")")"
    fi
    [ -n "$cited" ] || continue
    if [[ "$cited" = /* || "$cited" =~ ^[A-Za-z]: ]]; then
      continue
    fi
    line_number="$(grep -nF -- "$cited" "$ROOT/$source" | head -n 1 | cut -d: -f1)"
    checked=$((checked + 1))
    if ! resolved="$(normalize "$base/$cited")"; then
      echo "::error file=$source,line=${line_number:-1}::$source:${line_number:-1}: '$cited' resolves outside the repository"
      status=1
      continue
    fi
    if ! covered "$resolved"; then
      echo "::error file=$source,line=${line_number:-1}::$source:${line_number:-1}: build input '$resolved' (cited as '$cited') is not covered by the '$FILTER_NAME' path filter of .github/workflows/run_tests.yml"
      status=1
    fi
  done
done

if [ "$checked" -eq 0 ]; then
  echo "::error::no include_str!, include_bytes!, include!, #[folder] or rerun-if-changed path found, the coverage check scanned nothing"
  exit 1
fi

if [ "$status" -eq 0 ]; then
  echo "every build input path cited in ${#sources[@]} Rust file(s) is covered by the '$FILTER_NAME' filter ($checked citations checked against ${#patterns[@]} patterns)"
fi
exit "$status"
