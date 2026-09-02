#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
MANIFEST="$ROOT/native/libghostty/manifest.toml"
EDITS=()

usage() {
  cat <<'USAGE'
Usage: repin-libghostty-manifest.sh [options]

  --set <key>=<value>                       re-pin a preamble key
  --set-target <target>:<key>=<value>       re-pin a key inside [targets."<target>"]
  --set-license <component>:<key>=<value>   re-pin a key inside a [[licenses]] entry
  --manifest <path>                         operate on another manifest copy

Values are written verbatim between the existing quotes, so they must not
contain a double quote or a newline.
USAGE
}

while (($#)); do
  case "$1" in
    --set|--set-target|--set-license)
      [[ $# -ge 2 ]] || { echo "$1 requires an argument" >&2; exit 2; }
      EDITS+=("${1#--set}|$2")
      shift 2
      ;;
    --manifest)
      [[ $# -ge 2 ]] || { echo "--manifest requires a path" >&2; exit 2; }
      MANIFEST="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

[[ -f "$MANIFEST" ]] || { echo "manifest not found: $MANIFEST" >&2; exit 1; }
((${#EDITS[@]})) || { echo "no edit requested" >&2; usage >&2; exit 2; }

TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT

apply_edit() {
  local scope="$1" selector="$2" key="$3" value="$4"

  case "$value" in
    *\"*|*$'\n'*)
      echo "value for $key must not contain a quote or a newline" >&2
      return 1
      ;;
  esac

  awk -v scope="$scope" -v selector="$selector" -v key="$key" -v value="$value" '
    function flush_block(  i, line, replaced_here) {
      if (block_component == selector) {
        for (i = 1; i <= block_len; i++) {
          line = block[i]
          if (index(line, prefix) == 1 && substr(line, length(line)) == "\"") {
            block[i] = prefix value "\""
            replaced++
          }
        }
      }
      for (i = 1; i <= block_len; i++) print block[i]
      block_len = 0
      block_component = ""
    }
    BEGIN {
      prefix = key " = \""
      component_prefix = "component = \""
      in_scope = (scope == "preamble")
      in_block = 0
      block_len = 0
    }
    scope == "license" {
      if ($0 == "[[licenses]]") {
        if (in_block) flush_block()
        in_block = 1
        block_len = 1
        block[1] = $0
        next
      }
      if (in_block) {
        if (index($0, component_prefix) == 1 && substr($0, length($0)) == "\"") {
          block_component = substr($0, length(component_prefix) + 1, length($0) - length(component_prefix) - 1)
        }
        block[++block_len] = $0
        next
      }
      print
      next
    }
    {
      if (substr($0, 1, 1) == "[") {
        if (scope == "preamble") in_scope = 0
        else in_scope = ($0 == "[targets.\"" selector "\"]")
        print
        next
      }
      if (in_scope && index($0, prefix) == 1 && substr($0, length($0)) == "\"") {
        print prefix value "\""
        replaced++
        next
      }
      print
    }
    END {
      if (scope == "license" && in_block) flush_block()
      if (replaced != 1) {
        printf("expected exactly one %s match for %s, found %d\n", scope, key, replaced + 0) > "/dev/stderr"
        exit 1
      }
    }
  ' "$MANIFEST" > "$TMP"

  cat "$TMP" > "$MANIFEST"
}

for edit in "${EDITS[@]}"; do
  kind="${edit%%|*}"
  spec="${edit#*|}"
  case "$kind" in
    "")
      key="${spec%%=*}"
      [[ "$key" != "$spec" ]] || { echo "--set needs <key>=<value>: $spec" >&2; exit 2; }
      apply_edit preamble "" "$key" "${spec#*=}"
      ;;
    -target|-license)
      selector="${spec%%:*}"
      [[ "$selector" != "$spec" ]] || { echo "--set$kind needs <selector>:<key>=<value>: $spec" >&2; exit 2; }
      rest="${spec#*:}"
      key="${rest%%=*}"
      [[ "$key" != "$rest" ]] || { echo "--set$kind needs <selector>:<key>=<value>: $spec" >&2; exit 2; }
      apply_edit "${kind#-}" "$selector" "$key" "${rest#*=}"
      ;;
    *)
      echo "internal error: unknown edit kind $kind" >&2
      exit 1
      ;;
  esac
  echo "re-pinned ${spec%%=*}"
done
